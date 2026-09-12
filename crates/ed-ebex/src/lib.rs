//! Language-neutral EBEX container encoding, decoding and validation: the
//! one crate that owns the byte format. The sync protocol around it
//! (manifest, SHA-256 verification, resumable download) lives in `ed-sync`.

pub mod encode;
pub mod writer;

pub use encode::{
    build_string_table, community_section_plans, community_section_required, encode_availability,
    encode_market_auxiliary, encode_station_snapshots, encode_symbol_catalog, stars_section_plan,
    string_id, EncodedSymbolCatalog,
};
pub use writer::{compress_file, decompress_file, map_file, SectionPlan, SnapshotWriter};

use std::collections::{BTreeMap, BTreeSet};

use anyhow::{ensure, Context, Result};

pub const CONTAINER_VERSION: u16 = 1;
pub const HEADER_BYTES: usize = 64;
pub const DIRECTORY_ENTRY_BYTES: usize = 64;
pub const FLAG_FULL_BASELINE: u32 = 1;
pub const SECTION_SYSTEMS: u16 = 1;
pub const SECTION_STATIONS: u16 = 2;
pub const SECTION_COMMODITIES: u16 = 4;
pub const SECTION_MARKETS: u16 = 5;
pub const SECTION_MODULES: u16 = 6;
pub const SECTION_OUTFITTING: u16 = 7;
pub const SECTION_SHIPS: u16 = 8;
pub const SECTION_SHIPYARDS: u16 = 9;
/// Main-star classes learned from sources beyond the bootstrap dump (EDSM
/// bodies dumps, EDSM lookups, commanders' scans): one record per system,
/// the class of its main star. Published as its own product (`stars`) so
/// it can be refreshed weekly without a community baseline.
/// Station details (2026-09-04 addendum, lean v1): pads, carrier flag,
/// black-market presence, arrival distance, station type — everything
/// the Docked-event ingest learns and the identity sections lack.
/// Optional; grows fields via new schema versions as the data does.
pub const SECTION_STATION_DETAILS: u16 = 10;
/// (station, commodity) confiscation pairs, commodity ids from this
/// artifact's own commodity catalog. Optional (2026-09-04 addendum).
pub const SECTION_PROHIBITED: u16 = 11;
pub const SECTION_STARS: u16 = 16;
pub const SYSTEM_SCHEMA_V1: u16 = 1;
pub const STATION_SCHEMA_V1: u16 = 1;
pub const MARKET_SCHEMA_V1: u16 = 1;
pub const COMMODITY_SCHEMA_V1: u16 = 1;
pub const MODULE_SCHEMA_V1: u16 = 1;
pub const OUTFITTING_SCHEMA_V1: u16 = 1;
pub const SHIP_SCHEMA_V1: u16 = 1;
pub const SHIPYARD_SCHEMA_V1: u16 = 1;
pub const STAR_SCHEMA_V1: u16 = 1;
pub const STATION_DETAILS_SCHEMA_V1: u16 = 1;
pub const PROHIBITED_SCHEMA_V1: u16 = 1;
pub const SYSTEM_RECORD_BYTES: u32 = 80;
pub const STATION_RECORD_BYTES: u32 = 48;
pub const MARKET_RECORD_BYTES: u32 = 34;
pub const COMMODITY_RECORD_BYTES: u32 = 16;
pub const SYMBOL_CATALOG_RECORD_BYTES: u32 = 8;
pub const AVAILABILITY_RECORD_BYTES: u32 = 16;
pub const STAR_RECORD_BYTES: u32 = 24;
pub const STATION_DETAILS_RECORD_BYTES: u32 = 26;
pub const PROHIBITED_RECORD_BYTES: u32 = 12;
const MAGIC: &[u8; 8] = b"EBEX\0\0\0\0";

/// Every `(section, schema)` pair this crate can decode. A publisher
/// self-checks its output against this list.
pub const ALL_V1_SECTIONS: &[(u16, u16)] = &[
    (SECTION_SYSTEMS, SYSTEM_SCHEMA_V1),
    (SECTION_STATIONS, STATION_SCHEMA_V1),
    (SECTION_COMMODITIES, COMMODITY_SCHEMA_V1),
    (SECTION_MARKETS, MARKET_SCHEMA_V1),
    (SECTION_MODULES, MODULE_SCHEMA_V1),
    (SECTION_OUTFITTING, OUTFITTING_SCHEMA_V1),
    (SECTION_SHIPS, SHIP_SCHEMA_V1),
    (SECTION_SHIPYARDS, SHIPYARD_SCHEMA_V1),
    (SECTION_STATION_DETAILS, STATION_DETAILS_SCHEMA_V1),
    (SECTION_PROHIBITED, PROHIBITED_SCHEMA_V1),
];

/// What a client that hydrates only market data supports. Any other section
/// must be published as optional for such a client to accept the artifact.
pub const MARKET_BASELINE_SECTIONS: &[(u16, u16)] = &[(SECTION_MARKETS, MARKET_SCHEMA_V1)];

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Section {
    pub id: u16,
    pub schema: u16,
    pub required: bool,
    pub record_count: u64,
    pub record_size: u32,
    pub records: Vec<u8>,
    pub auxiliary: Vec<u8>,
}

/// Header fields a producer supplies. The section count is not an input:
/// it is derived from the sections actually encoded.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct SnapshotHeader {
    pub sequence: u64,
    pub created_at: i64,
    pub watermark: i64,
}

/// Header fields read back from a validated container.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct SnapshotMetadata {
    pub sequence: u64,
    pub created_at: i64,
    pub watermark: i64,
    pub section_count: u32,
}

impl SnapshotMetadata {
    pub fn header(&self) -> SnapshotHeader {
        SnapshotHeader {
            sequence: self.sequence,
            created_at: self.created_at,
            watermark: self.watermark,
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct MarketRecord {
    pub station_id: u64,
    pub commodity_id: u16,
    pub buy_price: u32,
    pub sell_price: u32,
    pub demand: u32,
    pub supply: u32,
    pub observed_at: i64,
}

/// Stars v1 (section 16), 24 bytes: system address, the main star's class
/// as `ed_galaxy::StarClass` code, whether it is scoopable, and when the
/// class was observed. Sorted by address, one record per system. The
/// address is `i64` like [`SystemRecord::address`] — negative addresses
/// mark provisional systems — so both sections sort the same key the
/// same way.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct StarRecord {
    pub address: i64,
    pub class: u8,
    pub scoopable: bool,
    pub observed_at: i64,
}

impl StarRecord {
    pub fn encode_into(self, output: &mut Vec<u8>) {
        output.extend_from_slice(&self.address.to_le_bytes());
        output.push(self.class);
        output.push(u8::from(self.scoopable));
        output.extend_from_slice(&[0u8; 6]);
        output.extend_from_slice(&self.observed_at.to_le_bytes());
    }

    pub fn decode(bytes: &[u8]) -> Result<Self> {
        ensure!(
            bytes.len() == STAR_RECORD_BYTES as usize,
            "invalid EBEX star record size"
        );
        ensure!(
            bytes[10..16].iter().all(|b| *b == 0),
            "reserved EBEX star record bytes are not zero"
        );
        ensure!(bytes[9] <= 1, "invalid EBEX star scoopable flag");
        Ok(Self {
            address: read_i64(bytes, 0)?,
            class: bytes[8],
            scoopable: bytes[9] == 1,
            observed_at: read_i64(bytes, 16)?,
        })
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct CommodityCatalogRecord {
    pub id: u16,
    pub symbol_id: u32,
    pub name_id: u32,
    pub category_id: u32,
}

impl CommodityCatalogRecord {
    pub fn encode_into(self, output: &mut Vec<u8>) {
        output.extend_from_slice(&self.id.to_le_bytes());
        output.extend_from_slice(&0_u16.to_le_bytes());
        output.extend_from_slice(&self.symbol_id.to_le_bytes());
        output.extend_from_slice(&self.name_id.to_le_bytes());
        output.extend_from_slice(&self.category_id.to_le_bytes());
    }

    pub fn decode(bytes: &[u8]) -> Result<Self> {
        ensure!(
            bytes.len() == COMMODITY_RECORD_BYTES as usize,
            "invalid EBEX commodity record size"
        );
        ensure!(
            read_u16(bytes, 2)? == 0,
            "nonzero EBEX commodity reserved bytes"
        );
        Ok(Self {
            id: read_u16(bytes, 0)?,
            symbol_id: read_u32(bytes, 4)?,
            name_id: read_u32(bytes, 8)?,
            category_id: read_u32(bytes, 12)?,
        })
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct SymbolCatalogRecord {
    pub id: u32,
    pub symbol_id: u32,
}

impl SymbolCatalogRecord {
    pub fn encode_into(self, output: &mut Vec<u8>) {
        output.extend_from_slice(&self.id.to_le_bytes());
        output.extend_from_slice(&self.symbol_id.to_le_bytes());
    }

    pub fn decode(bytes: &[u8]) -> Result<Self> {
        ensure!(
            bytes.len() == SYMBOL_CATALOG_RECORD_BYTES as usize,
            "invalid EBEX symbol catalog record size"
        );
        Ok(Self {
            id: read_u32(bytes, 0)?,
            symbol_id: read_u32(bytes, 4)?,
        })
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct AvailabilityRecord {
    pub station_id: u64,
    pub item_id: u32,
}

impl AvailabilityRecord {
    pub fn encode_into(self, output: &mut Vec<u8>) {
        output.extend_from_slice(&self.station_id.to_le_bytes());
        output.extend_from_slice(&self.item_id.to_le_bytes());
        output.extend_from_slice(&0_u32.to_le_bytes());
    }

    pub fn decode(bytes: &[u8]) -> Result<Self> {
        ensure!(
            bytes.len() == AVAILABILITY_RECORD_BYTES as usize,
            "invalid EBEX availability record size"
        );
        ensure!(
            read_u32(bytes, 12)? == 0,
            "nonzero EBEX availability reserved bytes"
        );
        Ok(Self {
            station_id: read_u64(bytes, 0)?,
            item_id: read_u32(bytes, 8)?,
        })
    }
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct SystemRecord {
    pub address: i64,
    pub x: f64,
    pub y: f64,
    pub z: f64,
    pub population: u64,
    pub observed_at: i64,
    pub name_id: u32,
    pub security_id: u32,
    pub allegiance_id: u32,
    pub controlling_power_id: u32,
    pub power_state_id: u32,
    pub powers_id: u32,
    pub flags: u32,
}

impl SystemRecord {
    pub const HAS_COORDINATES: u32 = 1;
    pub const HAS_POPULATION: u32 = 2;

    pub fn encode_into(self, output: &mut Vec<u8>) {
        output.extend_from_slice(&self.address.to_le_bytes());
        output.extend_from_slice(&self.x.to_le_bytes());
        output.extend_from_slice(&self.y.to_le_bytes());
        output.extend_from_slice(&self.z.to_le_bytes());
        output.extend_from_slice(&self.population.to_le_bytes());
        output.extend_from_slice(&self.observed_at.to_le_bytes());
        output.extend_from_slice(&self.name_id.to_le_bytes());
        output.extend_from_slice(&self.security_id.to_le_bytes());
        output.extend_from_slice(&self.allegiance_id.to_le_bytes());
        output.extend_from_slice(&self.controlling_power_id.to_le_bytes());
        output.extend_from_slice(&self.power_state_id.to_le_bytes());
        output.extend_from_slice(&self.powers_id.to_le_bytes());
        output.extend_from_slice(&self.flags.to_le_bytes());
        output.extend_from_slice(&0_u32.to_le_bytes());
    }

    pub fn decode(bytes: &[u8]) -> Result<Self> {
        ensure!(
            bytes.len() == SYSTEM_RECORD_BYTES as usize,
            "invalid EBEX system record size"
        );
        ensure!(
            read_u32(bytes, 76)? == 0,
            "nonzero EBEX system reserved bytes"
        );
        let flags = read_u32(bytes, 72)?;
        ensure!(
            flags & !(Self::HAS_COORDINATES | Self::HAS_POPULATION) == 0,
            "unknown EBEX system flags"
        );
        Ok(Self {
            address: read_i64(bytes, 0)?,
            x: read_f64(bytes, 8)?,
            y: read_f64(bytes, 16)?,
            z: read_f64(bytes, 24)?,
            population: read_u64(bytes, 32)?,
            observed_at: read_i64(bytes, 40)?,
            name_id: read_u32(bytes, 48)?,
            security_id: read_u32(bytes, 52)?,
            allegiance_id: read_u32(bytes, 56)?,
            controlling_power_id: read_u32(bytes, 60)?,
            power_state_id: read_u32(bytes, 64)?,
            powers_id: read_u32(bytes, 68)?,
            flags,
        })
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct StationRecord {
    pub id: u64,
    pub system_address: i64,
    pub name_id: u32,
    pub flags: u32,
    pub market_observed_at: i64,
    pub outfitting_observed_at: i64,
    pub shipyard_observed_at: i64,
}

impl StationRecord {
    pub const HAS_MARKET: u32 = 1;
    pub const HAS_OUTFITTING: u32 = 2;
    pub const HAS_SHIPYARD: u32 = 4;

    pub fn encode_into(self, output: &mut Vec<u8>) {
        output.extend_from_slice(&self.id.to_le_bytes());
        output.extend_from_slice(&self.system_address.to_le_bytes());
        output.extend_from_slice(&self.name_id.to_le_bytes());
        output.extend_from_slice(&self.flags.to_le_bytes());
        output.extend_from_slice(&self.market_observed_at.to_le_bytes());
        output.extend_from_slice(&self.outfitting_observed_at.to_le_bytes());
        output.extend_from_slice(&self.shipyard_observed_at.to_le_bytes());
    }

    pub fn decode(bytes: &[u8]) -> Result<Self> {
        ensure!(
            bytes.len() == STATION_RECORD_BYTES as usize,
            "invalid EBEX station record size"
        );
        let flags = read_u32(bytes, 20)?;
        ensure!(
            flags & !(Self::HAS_MARKET | Self::HAS_OUTFITTING | Self::HAS_SHIPYARD) == 0,
            "unknown EBEX station flags"
        );
        Ok(Self {
            id: read_u64(bytes, 0)?,
            system_address: read_i64(bytes, 8)?,
            name_id: read_u32(bytes, 16)?,
            flags,
            market_observed_at: read_i64(bytes, 24)?,
            outfitting_observed_at: read_i64(bytes, 32)?,
            shipyard_observed_at: read_i64(bytes, 40)?,
        })
    }
}

/// One station's details (addendum section 10, lean v1). `type_id`
/// resolves through the section's own string table; 0 means unknown.
/// Pads and arrival are only meaningful when their flags say so.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct StationDetailsRecord {
    pub station_id: u64,
    pub flags: u32,
    pub pad_small: u16,
    pub pad_medium: u16,
    pub pad_large: u16,
    pub arrival_ls: f32,
    pub type_id: u32,
}

impl StationDetailsRecord {
    pub const IS_CARRIER: u32 = 1;
    pub const HAS_BLACK_MARKET: u32 = 2;
    pub const HAS_PADS: u32 = 4;
    pub const HAS_ARRIVAL: u32 = 8;

    pub fn encode_into(self, output: &mut Vec<u8>) {
        output.extend_from_slice(&self.station_id.to_le_bytes());
        output.extend_from_slice(&self.flags.to_le_bytes());
        output.extend_from_slice(&self.pad_small.to_le_bytes());
        output.extend_from_slice(&self.pad_medium.to_le_bytes());
        output.extend_from_slice(&self.pad_large.to_le_bytes());
        output.extend_from_slice(&self.arrival_ls.to_le_bytes());
        output.extend_from_slice(&self.type_id.to_le_bytes());
    }

    pub fn decode(bytes: &[u8]) -> Result<Self> {
        ensure!(
            bytes.len() == STATION_DETAILS_RECORD_BYTES as usize,
            "invalid EBEX station details record size"
        );
        let flags = read_u32(bytes, 8)?;
        ensure!(
            flags
                & !(Self::IS_CARRIER | Self::HAS_BLACK_MARKET | Self::HAS_PADS | Self::HAS_ARRIVAL)
                == 0,
            "unknown EBEX station details flags"
        );
        Ok(Self {
            station_id: read_u64(bytes, 0)?,
            flags,
            pad_small: read_u16(bytes, 12)?,
            pad_medium: read_u16(bytes, 14)?,
            pad_large: read_u16(bytes, 16)?,
            arrival_ls: f32::from_le_bytes(bytes[18..22].try_into().expect("length checked above")),
            type_id: read_u32(bytes, 22)?,
        })
    }
}

/// One confiscation pair (addendum section 11): `commodity_id` refers to
/// the same artifact's commodity catalog.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ProhibitedRecord {
    pub station_id: u64,
    pub commodity_id: u32,
}

impl ProhibitedRecord {
    pub fn encode_into(self, output: &mut Vec<u8>) {
        output.extend_from_slice(&self.station_id.to_le_bytes());
        output.extend_from_slice(&self.commodity_id.to_le_bytes());
    }

    pub fn decode(bytes: &[u8]) -> Result<Self> {
        ensure!(
            bytes.len() == PROHIBITED_RECORD_BYTES as usize,
            "invalid EBEX prohibited record size"
        );
        Ok(Self {
            station_id: read_u64(bytes, 0)?,
            commodity_id: read_u32(bytes, 8)?,
        })
    }
}

pub fn station_details_records(
    section: SectionRef<'_>,
) -> Result<impl Iterator<Item = StationDetailsRecord> + '_> {
    ensure!(
        section.id == SECTION_STATION_DETAILS
            && section.schema == STATION_DETAILS_SCHEMA_V1
            && section.record_size == STATION_DETAILS_RECORD_BYTES
            && section.records.len() as u64
                == section.record_count * u64::from(STATION_DETAILS_RECORD_BYTES),
        "not a v1 EBEX station details section"
    );
    Ok(section
        .records
        .chunks(STATION_DETAILS_RECORD_BYTES as usize)
        .map(|bytes| StationDetailsRecord::decode(bytes).expect("size checked above")))
}

pub fn prohibited_records(
    section: SectionRef<'_>,
) -> Result<impl Iterator<Item = ProhibitedRecord> + '_> {
    ensure!(
        section.id == SECTION_PROHIBITED
            && section.schema == PROHIBITED_SCHEMA_V1
            && section.record_size == PROHIBITED_RECORD_BYTES
            && section.records.len() as u64
                == section.record_count * u64::from(PROHIBITED_RECORD_BYTES),
        "not a v1 EBEX prohibited section"
    );
    Ok(section
        .records
        .chunks(PROHIBITED_RECORD_BYTES as usize)
        .map(|bytes| ProhibitedRecord::decode(bytes).expect("size checked above")))
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct StringDefinition {
    pub id: u32,
    pub value: String,
}

impl MarketRecord {
    pub fn encode_into(self, output: &mut Vec<u8>) {
        output.extend_from_slice(&self.station_id.to_le_bytes());
        output.extend_from_slice(&self.commodity_id.to_le_bytes());
        output.extend_from_slice(&self.buy_price.to_le_bytes());
        output.extend_from_slice(&self.sell_price.to_le_bytes());
        output.extend_from_slice(&self.demand.to_le_bytes());
        output.extend_from_slice(&self.supply.to_le_bytes());
        output.extend_from_slice(&self.observed_at.to_le_bytes());
    }

    pub fn decode(bytes: &[u8]) -> Result<Self> {
        ensure!(
            bytes.len() == MARKET_RECORD_BYTES as usize,
            "invalid EBEX market record size"
        );
        Ok(Self {
            station_id: read_u64(bytes, 0)?,
            commodity_id: read_u16(bytes, 8)?,
            buy_price: read_u32(bytes, 10)?,
            sell_price: read_u32(bytes, 14)?,
            demand: read_u32(bytes, 18)?,
            supply: read_u32(bytes, 22)?,
            observed_at: read_i64(bytes, 26)?,
        })
    }
}

#[derive(Clone, Copy, Debug)]
pub struct SectionRef<'a> {
    pub id: u16,
    pub schema: u16,
    pub required: bool,
    pub record_count: u64,
    pub record_size: u32,
    pub records: &'a [u8],
    pub auxiliary: &'a [u8],
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CommodityDefinition {
    pub id: u16,
    pub symbol: String,
    pub name: String,
    pub category: String,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct MarketSnapshot {
    pub station_id: u64,
    pub observed_at: i64,
}

pub type StationSnapshot = MarketSnapshot;

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct MarketAuxiliary {
    pub commodities: Vec<CommodityDefinition>,
    pub stations: Vec<MarketSnapshot>,
}

/// Locate a validated section. Unknown optional sections can be skipped by
/// callers; required unknown sections must be rejected by the hydration layer.
pub fn section(bytes: &[u8], wanted: u16) -> Result<Option<SectionRef<'_>>> {
    Ok(sections(bytes)?
        .into_iter()
        .find(|section| section.id == wanted))
}

/// Enforce the container rule that unknown optional sections are skippable but
/// every required section must have an explicitly supported `(id, schema)`.
pub fn validate_required_sections(bytes: &[u8], supported: &[(u16, u16)]) -> Result<()> {
    for section in sections(bytes)? {
        ensure!(
            !section.required || supported.contains(&(section.id, section.schema)),
            "unsupported required EBEX section {} schema {}",
            section.id,
            section.schema
        );
    }
    Ok(())
}

pub fn sections(bytes: &[u8]) -> Result<Vec<SectionRef<'_>>> {
    let metadata = validate_snapshot(bytes)?;
    parse_directory(bytes, metadata.section_count)
}

/// [`sections`] for bytes the caller has ALREADY passed through
/// [`validate_snapshot`]: the directory parse alone, no re-walk of every
/// record and auxiliary byte. `sections()` itself re-validates, so a
/// hydrate that took validated bytes still paid full validation walks
/// through it — the 2026-09-04 review finding this closes. Every read
/// here stays bounds-checked, so unvalidated bytes error rather than
/// misbehave; only the validating entry points certify CONTENT.
pub fn sections_prevalidated(bytes: &[u8]) -> Result<Vec<SectionRef<'_>>> {
    ensure!(bytes.len() >= HEADER_BYTES, "truncated EBEX header");
    ensure!(&bytes[0..8] == MAGIC, "invalid EBEX magic");
    parse_directory(bytes, read_u32(bytes, 40)?)
}

fn parse_directory(bytes: &[u8], section_count: u32) -> Result<Vec<SectionRef<'_>>> {
    let mut found = Vec::with_capacity(usize::try_from(section_count)?);
    for index in 0..usize::try_from(section_count)? {
        let at = HEADER_BYTES + index * DIRECTORY_ENTRY_BYTES;
        let id = read_u16(bytes, at)?;
        let records = checked_region(bytes, read_u64(bytes, at + 24)?, read_u64(bytes, at + 32)?)?;
        let auxiliary = if read_u64(bytes, at + 48)? == 0 {
            0..0
        } else {
            checked_region(bytes, read_u64(bytes, at + 40)?, read_u64(bytes, at + 48)?)?
        };
        found.push(SectionRef {
            id,
            schema: read_u16(bytes, at + 2)?,
            required: read_u32(bytes, at + 4)? & 1 != 0,
            record_count: read_u64(bytes, at + 8)?,
            record_size: read_u32(bytes, at + 16)?,
            records: &bytes[records],
            auxiliary: &bytes[auxiliary],
        });
    }
    Ok(found)
}

pub fn system_records(section: SectionRef<'_>) -> Result<impl Iterator<Item = SystemRecord> + '_> {
    ensure!(section.id == SECTION_SYSTEMS, "not an EBEX systems section");
    ensure!(
        section.schema == SYSTEM_SCHEMA_V1,
        "unsupported EBEX systems schema {}",
        section.schema
    );
    ensure!(
        section.record_size == SYSTEM_RECORD_BYTES,
        "invalid EBEX system record size"
    );
    Ok(section
        .records
        .chunks_exact(SYSTEM_RECORD_BYTES as usize)
        .map(|record| SystemRecord::decode(record).expect("validated EBEX system record")))
}

pub fn station_records(
    section: SectionRef<'_>,
) -> Result<impl Iterator<Item = StationRecord> + '_> {
    ensure!(
        section.id == SECTION_STATIONS,
        "not an EBEX stations section"
    );
    ensure!(
        section.schema == STATION_SCHEMA_V1,
        "unsupported EBEX stations schema {}",
        section.schema
    );
    ensure!(
        section.record_size == STATION_RECORD_BYTES,
        "invalid EBEX station record size"
    );
    Ok(section
        .records
        .chunks_exact(STATION_RECORD_BYTES as usize)
        .map(|record| StationRecord::decode(record).expect("validated EBEX station record")))
}

pub fn string_table(section: SectionRef<'_>) -> Result<Vec<StringDefinition>> {
    let bytes = section.auxiliary;
    ensure!(bytes.len() >= 4, "truncated EBEX string table");
    let count = usize::try_from(read_u32(bytes, 0)?)?;
    let mut at = 4usize;
    let mut strings = Vec::with_capacity(count);
    let mut previous_id = 0u32;
    for _ in 0..count {
        ensure!(
            at.checked_add(8).is_some_and(|end| end <= bytes.len()),
            "truncated EBEX string entry"
        );
        let id = read_u32(bytes, at)?;
        ensure!(id > previous_id, "EBEX string ids are not strictly sorted");
        previous_id = id;
        let length = usize::try_from(read_u32(bytes, at + 4)?)?;
        at += 8;
        let end = at
            .checked_add(length)
            .context("EBEX string table overflow")?;
        let value = std::str::from_utf8(bytes.get(at..end).context("truncated EBEX string value")?)
            .context("invalid UTF-8 in EBEX string table")?
            .to_owned();
        ensure!(!value.is_empty(), "empty EBEX string value");
        strings.push(StringDefinition { id, value });
        at = end;
    }
    ensure!(
        at == bytes.len(),
        "unexpected EBEX string table trailing bytes"
    );
    Ok(strings)
}

pub fn commodity_records(
    section: SectionRef<'_>,
) -> Result<impl Iterator<Item = CommodityCatalogRecord> + '_> {
    ensure!(
        section.id == SECTION_COMMODITIES,
        "not an EBEX commodities section"
    );
    ensure!(
        section.schema == COMMODITY_SCHEMA_V1,
        "unsupported EBEX commodities schema {}",
        section.schema
    );
    ensure!(
        section.record_size == COMMODITY_RECORD_BYTES,
        "invalid EBEX commodity record size"
    );
    Ok(section
        .records
        .chunks_exact(COMMODITY_RECORD_BYTES as usize)
        .map(|record| {
            CommodityCatalogRecord::decode(record).expect("validated EBEX commodity record")
        }))
}

fn symbol_catalog_records(
    section: SectionRef<'_>,
    expected_id: u16,
    expected_schema: u16,
) -> Result<impl Iterator<Item = SymbolCatalogRecord> + '_> {
    ensure!(section.id == expected_id, "unexpected EBEX catalog section");
    ensure!(
        section.schema == expected_schema,
        "unsupported EBEX catalog schema {}",
        section.schema
    );
    ensure!(
        section.record_size == SYMBOL_CATALOG_RECORD_BYTES,
        "invalid EBEX symbol catalog record size"
    );
    Ok(section
        .records
        .chunks_exact(SYMBOL_CATALOG_RECORD_BYTES as usize)
        .map(|record| {
            SymbolCatalogRecord::decode(record).expect("validated EBEX symbol catalog record")
        }))
}

pub fn module_records(
    section: SectionRef<'_>,
) -> Result<impl Iterator<Item = SymbolCatalogRecord> + '_> {
    symbol_catalog_records(section, SECTION_MODULES, MODULE_SCHEMA_V1)
}

pub fn ship_records(
    section: SectionRef<'_>,
) -> Result<impl Iterator<Item = SymbolCatalogRecord> + '_> {
    symbol_catalog_records(section, SECTION_SHIPS, SHIP_SCHEMA_V1)
}

fn availability_records(
    section: SectionRef<'_>,
    expected_id: u16,
    expected_schema: u16,
) -> Result<impl Iterator<Item = AvailabilityRecord> + '_> {
    ensure!(
        section.id == expected_id,
        "unexpected EBEX availability section"
    );
    ensure!(
        section.schema == expected_schema,
        "unsupported EBEX availability schema {}",
        section.schema
    );
    ensure!(
        section.record_size == AVAILABILITY_RECORD_BYTES,
        "invalid EBEX availability record size"
    );
    Ok(section
        .records
        .chunks_exact(AVAILABILITY_RECORD_BYTES as usize)
        .map(|record| {
            AvailabilityRecord::decode(record).expect("validated EBEX availability record")
        }))
}

pub fn outfitting_records(
    section: SectionRef<'_>,
) -> Result<impl Iterator<Item = AvailabilityRecord> + '_> {
    availability_records(section, SECTION_OUTFITTING, OUTFITTING_SCHEMA_V1)
}

pub fn shipyard_records(
    section: SectionRef<'_>,
) -> Result<impl Iterator<Item = AvailabilityRecord> + '_> {
    availability_records(section, SECTION_SHIPYARDS, SHIPYARD_SCHEMA_V1)
}

pub fn station_snapshots(section: SectionRef<'_>) -> Result<Vec<StationSnapshot>> {
    ensure!(
        matches!(section.id, SECTION_OUTFITTING | SECTION_SHIPYARDS),
        "not an EBEX availability section"
    );
    let bytes = section.auxiliary;
    ensure!(
        bytes.len() >= 8,
        "truncated EBEX station snapshot directory"
    );
    let count = usize::try_from(read_u64(bytes, 0)?)?;
    let expected = 8usize
        .checked_add(
            count
                .checked_mul(16)
                .context("EBEX station snapshot directory overflow")?,
        )
        .context("EBEX station snapshot directory overflow")?;
    ensure!(
        expected == bytes.len(),
        "invalid EBEX station snapshot directory length"
    );
    let mut snapshots = Vec::with_capacity(count);
    let mut previous = None;
    let mut at = 8;
    for _ in 0..count {
        let station_id = read_u64(bytes, at)?;
        ensure!(
            previous.is_none_or(|value| value < station_id),
            "EBEX station snapshot ids are not strictly sorted"
        );
        previous = Some(station_id);
        snapshots.push(StationSnapshot {
            station_id,
            observed_at: read_i64(bytes, at + 8)?,
        });
        at += 16;
    }
    Ok(snapshots)
}

pub fn market_records(section: SectionRef<'_>) -> Result<impl Iterator<Item = MarketRecord> + '_> {
    ensure!(section.id == SECTION_MARKETS, "not an EBEX market section");
    ensure!(
        section.schema == MARKET_SCHEMA_V1,
        "unsupported EBEX market schema {}",
        section.schema
    );
    ensure!(
        section.record_size == MARKET_RECORD_BYTES,
        "invalid EBEX market record size"
    );
    Ok(section
        .records
        .chunks_exact(MARKET_RECORD_BYTES as usize)
        .map(|record| {
            // The section validator proved every chunk has the exact record size.
            MarketRecord::decode(record).expect("validated EBEX market record")
        }))
}

/// Decode the transitional market auxiliary dictionary. The final
/// multi-section artifact carries the same definitions in section 4.
pub fn market_auxiliary(section: SectionRef<'_>) -> Result<MarketAuxiliary> {
    ensure!(section.id == SECTION_MARKETS, "not an EBEX market section");
    let bytes = section.auxiliary;
    ensure!(bytes.len() >= 2, "truncated EBEX commodity dictionary");
    let count = usize::from(read_u16(bytes, 0)?);
    let mut at = 2usize;
    let mut commodities = Vec::with_capacity(count);
    let mut previous_id = 0u16;
    for _ in 0..count {
        ensure!(
            at.checked_add(8).is_some_and(|end| end <= bytes.len()),
            "truncated EBEX commodity dictionary entry"
        );
        let id = read_u16(bytes, at)?;
        ensure!(
            id > previous_id,
            "EBEX commodity ids are not strictly sorted"
        );
        previous_id = id;
        let lengths = [
            usize::from(read_u16(bytes, at + 2)?),
            usize::from(read_u16(bytes, at + 4)?),
            usize::from(read_u16(bytes, at + 6)?),
        ];
        at += 8;
        let mut fields = Vec::with_capacity(3);
        for length in lengths {
            let end = at
                .checked_add(length)
                .context("EBEX commodity dictionary overflow")?;
            let field = bytes
                .get(at..end)
                .context("truncated EBEX commodity dictionary field")?;
            fields.push(
                std::str::from_utf8(field)
                    .context("invalid UTF-8 in EBEX commodity dictionary")?
                    .to_owned(),
            );
            at = end;
        }
        ensure!(!fields[0].is_empty(), "empty EBEX commodity symbol");
        commodities.push(CommodityDefinition {
            id,
            symbol: fields.remove(0),
            name: fields.remove(0),
            category: fields.remove(0),
        });
    }
    ensure!(
        at.checked_add(8).is_some_and(|end| end <= bytes.len()),
        "truncated EBEX market snapshot directory"
    );
    let station_count = usize::try_from(read_u64(bytes, at)?)?;
    at += 8;
    let expected_end = at
        .checked_add(
            station_count
                .checked_mul(16)
                .context("EBEX market snapshot directory overflow")?,
        )
        .context("EBEX market snapshot directory overflow")?;
    ensure!(
        expected_end == bytes.len(),
        "invalid EBEX market snapshot directory length"
    );
    let mut stations = Vec::with_capacity(station_count);
    let mut previous_station = None;
    for _ in 0..station_count {
        let station_id = read_u64(bytes, at)?;
        ensure!(
            previous_station.is_none_or(|previous| station_id > previous),
            "EBEX market station ids are not strictly sorted"
        );
        previous_station = Some(station_id);
        stations.push(MarketSnapshot {
            station_id,
            observed_at: read_i64(bytes, at + 8)?,
        });
        at += 16;
    }
    Ok(MarketAuxiliary {
        commodities,
        stations,
    })
}

pub fn market_dictionary(section: SectionRef<'_>) -> Result<Vec<CommodityDefinition>> {
    Ok(market_auxiliary(section)?.commodities)
}

/// Iterate a stars-v1 section's records.
pub fn star_records(section: SectionRef<'_>) -> Result<impl Iterator<Item = StarRecord> + '_> {
    ensure!(section.id == SECTION_STARS, "not an EBEX stars section");
    ensure!(
        section.schema == STAR_SCHEMA_V1,
        "unsupported EBEX stars schema"
    );
    ensure!(
        section.record_size == STAR_RECORD_BYTES,
        "invalid EBEX stars record size"
    );
    Ok(section
        .records
        .chunks_exact(STAR_RECORD_BYTES as usize)
        .map(|chunk| StarRecord::decode(chunk).expect("record size checked")))
}

/// Validate a stars-v1 section: strictly sorted by address, reserved bytes
/// zero, one record per system.
pub fn validate_stars_section(section: SectionRef<'_>) -> Result<()> {
    ensure!(
        section.auxiliary.is_empty(),
        "EBEX stars section carries no auxiliary region"
    );
    let mut previous = None;
    for chunk in section.records.chunks_exact(STAR_RECORD_BYTES as usize) {
        let record = StarRecord::decode(chunk)?;
        ensure!(
            previous.is_none_or(|p| record.address > p),
            "EBEX star records are not strictly sorted"
        );
        previous = Some(record.address);
    }
    Ok(())
}

/// Validate the complete market-v1 relation, including ordering and every
/// reference carried by its auxiliary dictionaries.
pub fn validate_market_section(section: SectionRef<'_>) -> Result<()> {
    let auxiliary = market_auxiliary(section)?;
    let commodities = auxiliary
        .commodities
        .iter()
        .map(|commodity| commodity.id)
        .collect::<BTreeSet<_>>();
    let stations = auxiliary
        .stations
        .iter()
        .map(|station| (station.station_id, station.observed_at))
        .collect::<BTreeMap<_, _>>();
    let mut previous_key = None;
    for record in market_records(section)? {
        let key = (record.station_id, record.commodity_id);
        ensure!(
            previous_key.is_none_or(|previous| key > previous),
            "EBEX market records are not strictly sorted"
        );
        previous_key = Some(key);
        ensure!(
            commodities.contains(&record.commodity_id),
            "EBEX market row references unknown commodity"
        );
        let observed_at = stations
            .get(&record.station_id)
            .context("EBEX market row references unknown station snapshot")?;
        ensure!(
            *observed_at == record.observed_at,
            "EBEX market row timestamp differs from station snapshot"
        );
    }
    Ok(())
}

/// Everything a market-only client must check before touching its database:
/// container integrity, that every required section is one it supports, and
/// the full market relation. Returns the market section.
///
/// Exactly ONE container walk: the old body called
/// `validate_required_sections` and then `section()`, each of which
/// re-ran `validate_snapshot` — two silent extra walks inside a function
/// whose callers often had already walked once themselves (the
/// 2026-09-04 review's finding 1, recurring one layer up).
pub fn validate_market_baseline<'a>(
    bytes: &'a [u8],
    supported: &[(u16, u16)],
) -> Result<SectionRef<'a>> {
    validate_snapshot(bytes)?;
    market_baseline_prevalidated(bytes, supported)
}

/// [`validate_market_baseline`] for bytes the caller has ALREADY passed
/// through [`validate_snapshot`]: the section policy and the market
/// relation, with no container re-walk. The market-relation walk itself
/// is real content validation and always runs.
pub fn market_baseline_prevalidated<'a>(
    bytes: &'a [u8],
    supported: &[(u16, u16)],
) -> Result<SectionRef<'a>> {
    let sections = sections_prevalidated(bytes)?;
    for section in &sections {
        ensure!(
            !section.required || supported.contains(&(section.id, section.schema)),
            "unsupported required EBEX section {} schema {}",
            section.id,
            section.schema
        );
    }
    let market = sections
        .into_iter()
        .find(|section| section.id == SECTION_MARKETS)
        .context("EBEX has no market section")?;
    validate_market_section(market)?;
    Ok(market)
}

pub fn encode_snapshot(header: SnapshotHeader, mut sections: Vec<Section>) -> Result<Vec<u8>> {
    ensure!(
        !sections.is_empty(),
        "EBEX must contain at least one section"
    );
    sections.sort_by_key(|section| section.id);
    let unique = sections
        .iter()
        .map(|section| section.id)
        .collect::<BTreeSet<_>>();
    ensure!(unique.len() == sections.len(), "duplicate EBEX section id");
    for section in &sections {
        ensure!(
            section.id != 0 && section.schema != 0,
            "invalid EBEX section identity"
        );
        if section.record_size != 0 {
            let expected = section
                .record_count
                .checked_mul(u64::from(section.record_size))
                .context("EBEX section size overflow")?;
            ensure!(
                expected == section.records.len() as u64,
                "EBEX record length mismatch"
            );
        }
    }

    let directory_bytes = sections
        .len()
        .checked_mul(DIRECTORY_ENTRY_BYTES)
        .context("EBEX directory size overflow")?;
    let mut cursor = align8(HEADER_BYTES + directory_bytes)?;
    let mut locations = Vec::with_capacity(sections.len());
    for section in &sections {
        let record_offset = cursor;
        cursor = cursor
            .checked_add(section.records.len())
            .context("EBEX size overflow")?;
        let auxiliary_offset = if section.auxiliary.is_empty() {
            0
        } else {
            cursor = align8(cursor)?;
            cursor
        };
        cursor = cursor
            .checked_add(section.auxiliary.len())
            .context("EBEX size overflow")?;
        cursor = align8(cursor)?;
        locations.push((record_offset, auxiliary_offset));
    }

    let mut output = vec![0; cursor];
    output[0..8].copy_from_slice(MAGIC);
    put_u16(&mut output, 8, CONTAINER_VERSION);
    put_u16(&mut output, 10, HEADER_BYTES as u16);
    put_u32(&mut output, 12, FLAG_FULL_BASELINE);
    put_u64(&mut output, 16, header.sequence);
    put_i64(&mut output, 24, header.created_at);
    put_i64(&mut output, 32, header.watermark);
    put_u32(&mut output, 40, u32::try_from(sections.len())?);
    put_u16(&mut output, 44, DIRECTORY_ENTRY_BYTES as u16);
    put_u64(&mut output, 48, HEADER_BYTES as u64);

    for (index, section) in sections.iter().enumerate() {
        let directory = HEADER_BYTES + index * DIRECTORY_ENTRY_BYTES;
        let (record_offset, auxiliary_offset) = locations[index];
        put_u16(&mut output, directory, section.id);
        put_u16(&mut output, directory + 2, section.schema);
        put_u32(&mut output, directory + 4, u32::from(section.required));
        put_u64(&mut output, directory + 8, section.record_count);
        put_u32(&mut output, directory + 16, section.record_size);
        put_u64(&mut output, directory + 24, u64::try_from(record_offset)?);
        put_u64(
            &mut output,
            directory + 32,
            u64::try_from(section.records.len())?,
        );
        if auxiliary_offset != 0 {
            put_u64(
                &mut output,
                directory + 40,
                u64::try_from(auxiliary_offset)?,
            );
            put_u64(
                &mut output,
                directory + 48,
                u64::try_from(section.auxiliary.len())?,
            );
        }
        let mut checksum_input =
            Vec::with_capacity(section.records.len() + section.auxiliary.len());
        checksum_input.extend_from_slice(&section.records);
        checksum_input.extend_from_slice(&section.auxiliary);
        put_u32(&mut output, directory + 56, crc32c(&checksum_input));
        output[record_offset..record_offset + section.records.len()]
            .copy_from_slice(&section.records);
        if auxiliary_offset != 0 {
            output[auxiliary_offset..auxiliary_offset + section.auxiliary.len()]
                .copy_from_slice(&section.auxiliary);
        }
    }
    validate_snapshot(&output)?;
    Ok(output)
}

pub fn validate_snapshot(bytes: &[u8]) -> Result<SnapshotMetadata> {
    ensure!(bytes.len() >= HEADER_BYTES, "truncated EBEX header");
    ensure!(&bytes[0..8] == MAGIC, "invalid EBEX magic");
    ensure!(
        read_u16(bytes, 8)? == CONTAINER_VERSION,
        "unsupported EBEX version"
    );
    ensure!(
        read_u16(bytes, 10)? as usize == HEADER_BYTES,
        "invalid EBEX header size"
    );
    ensure!(
        read_u32(bytes, 12)? & !FLAG_FULL_BASELINE == 0,
        "unknown EBEX flags"
    );
    ensure!(
        bytes[46..48].iter().all(|byte| *byte == 0),
        "nonzero EBEX reserved bytes"
    );
    ensure!(
        bytes[56..64].iter().all(|byte| *byte == 0),
        "nonzero EBEX reserved bytes"
    );
    let count = read_u32(bytes, 40)?;
    ensure!(count > 0, "EBEX contains no sections");
    ensure!(
        read_u16(bytes, 44)? as usize == DIRECTORY_ENTRY_BYTES,
        "invalid EBEX directory entry size"
    );
    let directory_offset = usize::try_from(read_u64(bytes, 48)?)?;
    ensure!(
        directory_offset == HEADER_BYTES,
        "invalid EBEX directory offset"
    );
    let directory_end = directory_offset
        .checked_add(
            usize::try_from(count)?
                .checked_mul(DIRECTORY_ENTRY_BYTES)
                .context("EBEX directory overflow")?,
        )
        .context("EBEX directory overflow")?;
    ensure!(directory_end <= bytes.len(), "truncated EBEX directory");

    let mut previous_id = 0;
    let mut previous_end = align8(directory_end)?;
    for index in 0..usize::try_from(count)? {
        let at = directory_offset + index * DIRECTORY_ENTRY_BYTES;
        let id = read_u16(bytes, at)?;
        ensure!(id > previous_id, "EBEX sections are not strictly sorted");
        previous_id = id;
        ensure!(read_u16(bytes, at + 2)? != 0, "invalid EBEX section schema");
        ensure!(
            read_u32(bytes, at + 4)? & !1 == 0,
            "unknown EBEX section flags"
        );
        ensure!(
            bytes[at + 20..at + 24].iter().all(|byte| *byte == 0),
            "nonzero EBEX section reserved bytes"
        );
        ensure!(
            bytes[at + 60..at + 64].iter().all(|byte| *byte == 0),
            "nonzero EBEX section reserved bytes"
        );
        let records = checked_region(bytes, read_u64(bytes, at + 24)?, read_u64(bytes, at + 32)?)?;
        ensure!(records.start >= previous_end, "overlapping EBEX sections");
        ensure!(
            bytes[previous_end..records.start]
                .iter()
                .all(|byte| *byte == 0),
            "nonzero EBEX padding"
        );
        let record_size = read_u32(bytes, at + 16)?;
        if record_size != 0 {
            let expected = read_u64(bytes, at + 8)?
                .checked_mul(u64::from(record_size))
                .context("EBEX record size overflow")?;
            ensure!(
                expected == records.len() as u64,
                "EBEX record length mismatch"
            );
        }
        let auxiliary_length = read_u64(bytes, at + 48)?;
        let auxiliary = if auxiliary_length == 0 {
            ensure!(
                read_u64(bytes, at + 40)? == 0,
                "invalid empty EBEX auxiliary region"
            );
            0..0
        } else {
            let region = checked_region(bytes, read_u64(bytes, at + 40)?, auxiliary_length)?;
            ensure!(
                region.start >= records.end,
                "overlapping EBEX section regions"
            );
            region
        };
        let mut checksum_input = Vec::with_capacity(records.len() + auxiliary.len());
        checksum_input.extend_from_slice(&bytes[records.clone()]);
        checksum_input.extend_from_slice(&bytes[auxiliary.clone()]);
        ensure!(
            crc32c(&checksum_input) == read_u32(bytes, at + 56)?,
            "EBEX section checksum mismatch"
        );
        let data_end = records.end.max(auxiliary.end);
        previous_end = align8(data_end)?;
        ensure!(previous_end <= bytes.len(), "truncated EBEX padding");
        ensure!(
            bytes[data_end..previous_end].iter().all(|byte| *byte == 0),
            "nonzero EBEX padding"
        );
    }
    ensure!(
        previous_end == bytes.len(),
        "unexpected EBEX trailing bytes"
    );

    Ok(SnapshotMetadata {
        sequence: read_u64(bytes, 16)?,
        created_at: read_i64(bytes, 24)?,
        watermark: read_i64(bytes, 32)?,
        section_count: count,
    })
}

pub fn compress(bytes: &[u8], level: i32) -> Result<Vec<u8>> {
    zstd::stream::encode_all(bytes, level).context("compressing EBEX")
}

pub fn decompress(bytes: &[u8]) -> Result<Vec<u8>> {
    zstd::stream::decode_all(bytes).context("decompressing EBEX")
}

/// Stream-decompress `source` into `target` with the same progress
/// contract as [`decompress_with_progress`], holding only buffers in
/// memory — the client path for big snapshots (maintainer ruling 2026-09-04:
/// a multi-GB decode buffer in RAM is unacceptable; decompress to disk,
/// mmap, delete after use). Returns decompressed bytes written.
pub fn decompress_file_with_progress(
    source: &std::path::Path,
    target: &std::path::Path,
    mut on_progress: impl FnMut(u64),
) -> Result<u64> {
    use std::io::{BufReader, BufWriter, Read, Write};
    const EVERY: u64 = 8 << 20;
    struct Counting<R, F: FnMut(u64)> {
        inner: R,
        consumed: u64,
        reported: u64,
        report: F,
    }
    impl<R: Read, F: FnMut(u64)> Read for Counting<R, F> {
        fn read(&mut self, buf: &mut [u8]) -> std::io::Result<usize> {
            let n = self.inner.read(buf)?;
            self.consumed += n as u64;
            if self.consumed - self.reported >= EVERY {
                self.reported = self.consumed;
                (self.report)(self.consumed);
            }
            Ok(n)
        }
    }
    let total = std::fs::metadata(source)?.len();
    let input = BufReader::with_capacity(
        1 << 20,
        std::fs::File::open(source).with_context(|| format!("opening {}", source.display()))?,
    );
    let mut reader = Counting {
        inner: input,
        consumed: 0,
        reported: 0,
        report: &mut on_progress,
    };
    let mut decoder = zstd::stream::Decoder::new(&mut reader).context("starting zstd")?;
    let mut output = BufWriter::with_capacity(
        1 << 20,
        std::fs::File::create(target).with_context(|| format!("creating {}", target.display()))?,
    );
    let bytes = std::io::copy(&mut decoder, &mut output).context("decompressing EBEX")?;
    output.flush()?;
    on_progress(total);
    Ok(bytes)
}

/// [`decompress`], reporting compressed bytes consumed as the decoder
/// eats the input — REAL progress for a phase that otherwise sits silent
/// for minutes on a big snapshot (item 47: monitor everything, guess
/// nothing). The callback fires about every 8 MiB and once at the end.
pub fn decompress_with_progress(bytes: &[u8], mut on_progress: impl FnMut(u64)) -> Result<Vec<u8>> {
    const EVERY: u64 = 8 << 20;
    struct Counting<'a, F: FnMut(u64)> {
        inner: &'a [u8],
        consumed: u64,
        reported: u64,
        report: F,
    }
    impl<F: FnMut(u64)> std::io::Read for Counting<'_, F> {
        fn read(&mut self, buf: &mut [u8]) -> std::io::Result<usize> {
            let n = std::io::Read::read(&mut self.inner, buf)?;
            self.consumed += n as u64;
            if self.consumed - self.reported >= EVERY {
                self.reported = self.consumed;
                (self.report)(self.consumed);
            }
            Ok(n)
        }
    }
    let total = bytes.len() as u64;
    let mut reader = Counting {
        inner: bytes,
        consumed: 0,
        reported: 0,
        report: &mut on_progress,
    };
    let mut out = Vec::new();
    std::io::Read::read_to_end(
        &mut zstd::stream::Decoder::new(&mut reader).context("starting zstd")?,
        &mut out,
    )
    .context("decompressing EBEX")?;
    on_progress(total);
    Ok(out)
}

fn checked_region(bytes: &[u8], offset: u64, length: u64) -> Result<std::ops::Range<usize>> {
    let start = usize::try_from(offset)?;
    let end = start
        .checked_add(usize::try_from(length)?)
        .context("EBEX region overflow")?;
    ensure!(end <= bytes.len(), "EBEX region outside file");
    Ok(start..end)
}

fn align8(value: usize) -> Result<usize> {
    value
        .checked_add(7)
        .map(|value| value & !7)
        .context("EBEX alignment overflow")
}

fn put_u16(bytes: &mut [u8], at: usize, value: u16) {
    bytes[at..at + 2].copy_from_slice(&value.to_le_bytes());
}
fn put_u32(bytes: &mut [u8], at: usize, value: u32) {
    bytes[at..at + 4].copy_from_slice(&value.to_le_bytes());
}
fn put_u64(bytes: &mut [u8], at: usize, value: u64) {
    bytes[at..at + 8].copy_from_slice(&value.to_le_bytes());
}
fn put_i64(bytes: &mut [u8], at: usize, value: i64) {
    bytes[at..at + 8].copy_from_slice(&value.to_le_bytes());
}

fn read_u16(bytes: &[u8], at: usize) -> Result<u16> {
    Ok(u16::from_le_bytes(read_array(bytes, at)?))
}
fn read_u32(bytes: &[u8], at: usize) -> Result<u32> {
    Ok(u32::from_le_bytes(read_array(bytes, at)?))
}
fn read_u64(bytes: &[u8], at: usize) -> Result<u64> {
    Ok(u64::from_le_bytes(read_array(bytes, at)?))
}
fn read_i64(bytes: &[u8], at: usize) -> Result<i64> {
    Ok(i64::from_le_bytes(read_array(bytes, at)?))
}
fn read_f64(bytes: &[u8], at: usize) -> Result<f64> {
    Ok(f64::from_le_bytes(read_array(bytes, at)?))
}

fn read_array<const N: usize>(bytes: &[u8], at: usize) -> Result<[u8; N]> {
    bytes
        .get(at..at + N)
        .context("truncated EBEX field")?
        .try_into()
        .context("invalid EBEX field")
}

/// Incremental CRC-32C (Castagnoli): reflected polynomial 0x82F63B78,
/// initial value 0xFFFFFFFF, final XOR 0xFFFFFFFF. The one implementation
/// both the in-memory encoder and the streaming writer use.
#[derive(Clone, Copy)]
pub(crate) struct Crc32c(u32);

impl Crc32c {
    pub(crate) fn new() -> Self {
        Crc32c(!0)
    }
    pub(crate) fn update(&mut self, bytes: &[u8]) {
        let mut crc = self.0;
        for byte in bytes {
            crc ^= u32::from(*byte);
            for _ in 0..8 {
                crc = (crc >> 1) ^ (0x82f6_3b78_u32 & (0_u32.wrapping_sub(crc & 1)));
            }
        }
        self.0 = crc;
    }
    pub(crate) fn finish(self) -> u32 {
        !self.0
    }
}

fn crc32c(bytes: &[u8]) -> u32 {
    let mut crc = Crc32c::new();
    crc.update(bytes);
    crc.finish()
}

#[cfg(test)]
mod tests {
    /// A stars section round-trips, is validated for order and reserved
    /// bytes, and rides in a container the market-only reader skips.
    #[test]
    fn star_records_round_trip_and_validate() {
        let mut records = Vec::new();
        // A provisional (negative) address is legal and sorts first, signed.
        for (address, class, scoopable) in [
            (-7i64, 2u8, false),
            (5, 3, true),
            (9, 14, false),
            (10_477_373_803, 1, true),
        ] {
            StarRecord {
                address,
                class,
                scoopable,
                observed_at: 1_700_000_000,
            }
            .encode_into(&mut records);
        }
        assert_eq!(records.len(), 4 * STAR_RECORD_BYTES as usize);
        let bytes = encode_snapshot(
            SnapshotHeader {
                sequence: 1,
                created_at: 1,
                watermark: 1,
            },
            vec![Section {
                id: SECTION_STARS,
                schema: STAR_SCHEMA_V1,
                required: false,
                record_count: 4,
                record_size: STAR_RECORD_BYTES,
                records: records.clone(),
                auxiliary: vec![],
            }],
        )
        .unwrap();
        let stars = section(&bytes, SECTION_STARS).unwrap().unwrap();
        validate_stars_section(stars).unwrap();
        let decoded: Vec<StarRecord> = star_records(stars).unwrap().collect();
        assert_eq!(decoded[0].address, -7);
        assert_eq!(
            decoded[2],
            StarRecord {
                address: 9,
                class: 14,
                scoopable: false,
                observed_at: 1_700_000_000
            }
        );
        assert_eq!(decoded[3].address, 10_477_373_803);
        // Optional: a client that only knows the market baseline accepts the container.
        validate_required_sections(&bytes, MARKET_BASELINE_SECTIONS).unwrap();
        // Out of order is refused.
        let mut swapped = Vec::new();
        StarRecord {
            address: 9,
            class: 1,
            scoopable: true,
            observed_at: 1,
        }
        .encode_into(&mut swapped);
        StarRecord {
            address: 5,
            class: 1,
            scoopable: true,
            observed_at: 1,
        }
        .encode_into(&mut swapped);
        let bad = encode_snapshot(
            SnapshotHeader {
                sequence: 1,
                created_at: 1,
                watermark: 1,
            },
            vec![Section {
                id: SECTION_STARS,
                schema: STAR_SCHEMA_V1,
                required: false,
                record_count: 2,
                record_size: STAR_RECORD_BYTES,
                records: swapped,
                auxiliary: vec![],
            }],
        )
        .unwrap();
        assert!(validate_stars_section(section(&bad, SECTION_STARS).unwrap().unwrap()).is_err());
        // Reserved bytes must be zero.
        let mut dirty = records.clone();
        dirty[12] = 7;
        assert!(StarRecord::decode(&dirty[..STAR_RECORD_BYTES as usize]).is_err());
    }

    use super::*;
    use sha2::{Digest, Sha256};

    fn market_auxiliary_bytes() -> Vec<u8> {
        let mut auxiliary = Vec::new();
        auxiliary.extend_from_slice(&1_u16.to_le_bytes());
        auxiliary.extend_from_slice(&1_u16.to_le_bytes());
        auxiliary.extend_from_slice(&4_u16.to_le_bytes());
        auxiliary.extend_from_slice(&4_u16.to_le_bytes());
        auxiliary.extend_from_slice(&6_u16.to_le_bytes());
        auxiliary.extend_from_slice(b"goldGoldMetals");
        auxiliary.extend_from_slice(&1_u64.to_le_bytes());
        auxiliary.extend_from_slice(&7_u64.to_le_bytes());
        auxiliary.extend_from_slice(&6_i64.to_le_bytes());
        auxiliary
    }

    fn golden_hex() -> Vec<u8> {
        include_str!("../fixtures/ebex-v1-market.hex")
            .lines()
            .filter(|line| !line.trim_start().starts_with('#'))
            .flat_map(|line| line.split_whitespace())
            .flat_map(|word| word.as_bytes().chunks_exact(2))
            .map(|pair| {
                let digit = |byte: u8| match byte {
                    b'0'..=b'9' => byte - b'0',
                    b'a'..=b'f' => byte - b'a' + 10,
                    b'A'..=b'F' => byte - b'A' + 10,
                    _ => panic!("invalid golden hex digit"),
                };
                digit(pair[0]) << 4 | digit(pair[1])
            })
            .collect()
    }

    fn snapshot() -> Vec<u8> {
        let mut records = Vec::new();
        MarketRecord {
            station_id: 7,
            commodity_id: 1,
            buy_price: 2,
            sell_price: 3,
            demand: 4,
            supply: 5,
            observed_at: 6,
        }
        .encode_into(&mut records);
        encode_snapshot(
            SnapshotHeader {
                sequence: 9,
                created_at: 10,
                watermark: 8,
            },
            vec![Section {
                id: SECTION_MARKETS,
                schema: MARKET_SCHEMA_V1,
                required: true,
                record_count: 1,
                record_size: MARKET_RECORD_BYTES,
                records,
                auxiliary: market_auxiliary_bytes(),
            }],
        )
        .unwrap()
    }

    #[test]
    fn golden_market_v1_is_byte_exact_and_decodes_as_documented() {
        let bytes = snapshot();
        let golden = golden_hex();
        assert_eq!(bytes, golden);

        let expectation: serde_json::Value =
            serde_json::from_str(include_str!("../fixtures/ebex-v1-market.json")).unwrap();
        assert_eq!(
            bytes.len() as u64,
            expectation["uncompressed_bytes"].as_u64().unwrap()
        );
        assert_eq!(
            format!("{:x}", Sha256::digest(&bytes)),
            expectation["uncompressed_sha256"].as_str().unwrap()
        );

        let metadata = validate_snapshot(&bytes).unwrap();
        assert_eq!(
            metadata.sequence,
            expectation["metadata"]["sequence"].as_u64().unwrap()
        );
        assert_eq!(
            metadata.created_at,
            expectation["metadata"]["created_at"].as_i64().unwrap()
        );
        assert_eq!(
            metadata.watermark,
            expectation["metadata"]["watermark"].as_i64().unwrap()
        );
        let market = section(&bytes, SECTION_MARKETS).unwrap().unwrap();
        validate_market_section(market).unwrap();
        assert_eq!(
            market_records(market).unwrap().collect::<Vec<_>>(),
            vec![MarketRecord {
                station_id: 7,
                commodity_id: 1,
                buy_price: 2,
                sell_price: 3,
                demand: 4,
                supply: 5,
                observed_at: 6,
            }]
        );
        assert_eq!(
            market_auxiliary(market).unwrap(),
            MarketAuxiliary {
                commodities: vec![CommodityDefinition {
                    id: 1,
                    symbol: "gold".into(),
                    name: "Gold".into(),
                    category: "Metals".into(),
                }],
                stations: vec![MarketSnapshot {
                    station_id: 7,
                    observed_at: 6
                }],
            }
        );
    }

    #[test]
    fn golden_encoding_is_deterministic() {
        assert_eq!(snapshot(), snapshot());
        assert_eq!(snapshot(), golden_hex());
    }

    #[test]
    fn full_v1_golden_decodes_every_implemented_section() {
        let bytes = include_str!("../fixtures/ebex-v1-full.hex")
            .lines()
            .filter(|line| !line.trim_start().starts_with('#'))
            .flat_map(|line| line.split_whitespace())
            .flat_map(|word| word.as_bytes().chunks_exact(2))
            .map(|pair| {
                let digit = |byte: u8| match byte {
                    b'0'..=b'9' => byte - b'0',
                    b'a'..=b'f' => byte - b'a' + 10,
                    b'A'..=b'F' => byte - b'A' + 10,
                    _ => panic!("invalid golden hex digit"),
                };
                digit(pair[0]) << 4 | digit(pair[1])
            })
            .collect::<Vec<_>>();
        let expected: serde_json::Value =
            serde_json::from_str(include_str!("../fixtures/ebex-v1-full.json")).unwrap();
        assert_eq!(
            bytes.len() as u64,
            expected["uncompressed_bytes"].as_u64().unwrap()
        );
        assert_eq!(
            format!("{:x}", Sha256::digest(&bytes)),
            expected["uncompressed_sha256"]
        );
        let metadata = validate_snapshot(&bytes).unwrap();
        assert_eq!(metadata.sequence, 100);
        assert_eq!(metadata.created_at, 200);
        assert_eq!(metadata.watermark, 150);
        let found = sections(&bytes).unwrap();
        assert_eq!(
            found.iter().map(|section| section.id).collect::<Vec<_>>(),
            vec![1, 2, 4, 5, 6, 7, 8, 9, 16]
        );
        // The publication policy is baked into the fixture: only markets is
        // required, so a market-only client accepts the whole container.
        validate_required_sections(&bytes, MARKET_BASELINE_SECTIONS).unwrap();
        assert_eq!(
            found
                .iter()
                .filter(|s| s.required)
                .map(|s| s.id)
                .collect::<Vec<_>>(),
            vec![SECTION_MARKETS]
        );

        let systems = section(&bytes, SECTION_SYSTEMS).unwrap().unwrap();
        assert_eq!(system_records(systems).unwrap().count(), 1);
        assert_eq!(string_table(systems).unwrap().last().unwrap().value, "Sol");
        let stations = section(&bytes, SECTION_STATIONS).unwrap().unwrap();
        assert_eq!(station_records(stations).unwrap().next().unwrap().id, 10);
        assert_eq!(string_table(stations).unwrap()[0].value, "Galileo");

        let commodities = section(&bytes, SECTION_COMMODITIES).unwrap().unwrap();
        assert_eq!(
            commodity_records(commodities).unwrap().next().unwrap(),
            CommodityCatalogRecord {
                id: 1,
                symbol_id: 3,
                name_id: 1,
                category_id: 2,
            }
        );
        let market = section(&bytes, SECTION_MARKETS).unwrap().unwrap();
        validate_market_section(market).unwrap();
        assert_eq!(
            market_records(market).unwrap().next().unwrap().sell_price,
            60_000
        );

        let modules = section(&bytes, SECTION_MODULES).unwrap().unwrap();
        assert_eq!(module_records(modules).unwrap().next().unwrap().id, 1);
        assert_eq!(
            string_table(modules).unwrap()[0].value,
            "int_hyperdrive_size2_class1"
        );
        let outfitting = section(&bytes, SECTION_OUTFITTING).unwrap().unwrap();
        assert_eq!(
            outfitting_records(outfitting).unwrap().next().unwrap(),
            AvailabilityRecord {
                station_id: 10,
                item_id: 1
            }
        );
        assert_eq!(station_snapshots(outfitting).unwrap()[0].observed_at, 150);

        let ships = section(&bytes, SECTION_SHIPS).unwrap().unwrap();
        assert_eq!(ship_records(ships).unwrap().next().unwrap().id, 1);
        assert_eq!(string_table(ships).unwrap()[0].value, "cobramkiii");
        let shipyards = section(&bytes, SECTION_SHIPYARDS).unwrap().unwrap();
        assert_eq!(
            shipyard_records(shipyards).unwrap().next().unwrap(),
            AvailabilityRecord {
                station_id: 10,
                item_id: 1
            }
        );
        assert_eq!(station_snapshots(shipyards).unwrap()[0].observed_at, 150);

        let stars = section(&bytes, SECTION_STARS).unwrap().unwrap();
        validate_stars_section(stars).unwrap();
        let star_list: Vec<StarRecord> = star_records(stars).unwrap().collect();
        assert_eq!(
            star_list,
            vec![
                StarRecord {
                    address: -42,
                    class: 2,
                    scoopable: true,
                    observed_at: 150
                },
                StarRecord {
                    address: 10_477_373_803,
                    class: 14,
                    scoopable: false,
                    observed_at: 150
                },
            ],
            "signed addresses: the provisional star sorts first"
        );
    }

    #[test]
    fn round_trips_container_metadata() {
        let bytes = snapshot();
        assert_eq!(&bytes[..8], b"EBEX\0\0\0\0");
        assert_eq!(
            validate_snapshot(&bytes).unwrap(),
            SnapshotMetadata {
                sequence: 9,
                created_at: 10,
                watermark: 8,
                section_count: 1
            }
        );
        assert_eq!(decompress(&compress(&bytes, 1).unwrap()).unwrap(), bytes);
    }

    #[test]
    fn rejects_truncation_and_corruption() {
        let bytes = snapshot();
        for length in 0..bytes.len() {
            assert!(
                validate_snapshot(&bytes[..length]).is_err(),
                "accepted {length}-byte truncation"
            );
        }
        let mut corrupt = bytes.clone();
        *corrupt.last_mut().unwrap() ^= 1;
        assert!(validate_snapshot(&corrupt).is_err());

        let mut overflow = bytes.clone();
        put_u64(&mut overflow, HEADER_BYTES + 8, u64::MAX);
        assert!(validate_snapshot(&overflow).is_err());

        let mut unsupported = bytes;
        put_u16(&mut unsupported, 8, CONTAINER_VERSION + 1);
        assert!(validate_snapshot(&unsupported).is_err());
    }

    #[test]
    fn rejects_invalid_utf8_after_container_checksum_passes() {
        let mut bytes = snapshot();
        let records = 128..162;
        let auxiliary = 168..216;
        bytes[178] = 0xff;
        let mut checksum_input = bytes[records].to_vec();
        checksum_input.extend_from_slice(&bytes[auxiliary]);
        put_u32(&mut bytes, HEADER_BYTES + 56, crc32c(&checksum_input));
        validate_snapshot(&bytes).unwrap();
        let market = section(&bytes, SECTION_MARKETS).unwrap().unwrap();
        assert!(market_auxiliary(market).is_err());
    }

    #[test]
    fn required_unknown_sections_are_rejected_and_optional_ones_are_skipped() {
        let make = |required| {
            encode_snapshot(
                SnapshotHeader {
                    sequence: 1,
                    created_at: 2,
                    watermark: 3,
                },
                vec![Section {
                    id: 99,
                    schema: 7,
                    required,
                    record_count: 0,
                    record_size: 0,
                    records: Vec::new(),
                    auxiliary: Vec::new(),
                }],
            )
            .unwrap()
        };
        validate_required_sections(&make(false), &[(SECTION_MARKETS, MARKET_SCHEMA_V1)]).unwrap();
        assert!(
            validate_required_sections(&make(true), &[(SECTION_MARKETS, MARKET_SCHEMA_V1)])
                .is_err()
        );
    }

    #[test]
    fn unsupported_required_schema_is_rejected_by_shared_policy() {
        let mut bytes = snapshot();
        put_u16(&mut bytes, HEADER_BYTES + 2, MARKET_SCHEMA_V1 + 1);
        validate_snapshot(&bytes).unwrap();
        assert!(
            validate_required_sections(&bytes, &[(SECTION_MARKETS, MARKET_SCHEMA_V1)]).is_err()
        );
        let market = section(&bytes, SECTION_MARKETS).unwrap().unwrap();
        assert!(market_records(market).is_err());
    }

    #[test]
    fn crc32c_matches_standard_vector() {
        assert_eq!(crc32c(b"123456789"), 0xe306_9283);
    }

    #[test]
    fn identity_records_and_string_tables_round_trip() {
        let system = SystemRecord {
            address: -1,
            x: 1.0,
            y: 2.0,
            z: 3.0,
            population: 4,
            observed_at: 5,
            name_id: 1,
            security_id: 0,
            allegiance_id: 0,
            controlling_power_id: 0,
            power_state_id: 0,
            powers_id: 0,
            flags: SystemRecord::HAS_COORDINATES | SystemRecord::HAS_POPULATION,
        };
        let station = StationRecord {
            id: 7,
            system_address: -1,
            name_id: 1,
            flags: StationRecord::HAS_MARKET,
            market_observed_at: 8,
            outfitting_observed_at: 0,
            shipyard_observed_at: 0,
        };
        let mut system_bytes = Vec::new();
        system.encode_into(&mut system_bytes);
        let mut station_bytes = Vec::new();
        station.encode_into(&mut station_bytes);
        assert_eq!(SystemRecord::decode(&system_bytes).unwrap(), system);
        assert_eq!(StationRecord::decode(&station_bytes).unwrap(), station);

        let mut strings = Vec::new();
        strings.extend_from_slice(&1_u32.to_le_bytes());
        strings.extend_from_slice(&1_u32.to_le_bytes());
        strings.extend_from_slice(&3_u32.to_le_bytes());
        strings.extend_from_slice(b"Sol");
        let section = SectionRef {
            id: SECTION_SYSTEMS,
            schema: SYSTEM_SCHEMA_V1,
            required: true,
            record_count: 1,
            record_size: SYSTEM_RECORD_BYTES,
            records: &system_bytes,
            auxiliary: &strings,
        };
        assert_eq!(
            string_table(section).unwrap(),
            vec![StringDefinition {
                id: 1,
                value: "Sol".into()
            }]
        );
        assert_eq!(
            system_records(section).unwrap().collect::<Vec<_>>(),
            vec![system]
        );
    }

    #[test]
    fn shared_string_table_rejects_invalid_utf8_and_bad_lengths() {
        let mut invalid_utf8 = Vec::new();
        invalid_utf8.extend_from_slice(&1_u32.to_le_bytes());
        invalid_utf8.extend_from_slice(&1_u32.to_le_bytes());
        invalid_utf8.extend_from_slice(&1_u32.to_le_bytes());
        invalid_utf8.push(0xff);
        let section = SectionRef {
            id: SECTION_SYSTEMS,
            schema: SYSTEM_SCHEMA_V1,
            required: true,
            record_count: 0,
            record_size: SYSTEM_RECORD_BYTES,
            records: &[],
            auxiliary: &invalid_utf8,
        };
        assert!(string_table(section).is_err());
        let mut bad_length = invalid_utf8.clone();
        bad_length[8..12].copy_from_slice(&u32::MAX.to_le_bytes());
        assert!(string_table(SectionRef {
            id: SECTION_SYSTEMS,
            schema: SYSTEM_SCHEMA_V1,
            required: true,
            record_count: 0,
            record_size: SYSTEM_RECORD_BYTES,
            records: &[],
            auxiliary: &bad_length,
        })
        .is_err());
    }

    #[test]
    fn every_v1_record_decoder_rejects_reserved_bits_and_wrong_sizes() {
        let mut system = vec![0; SYSTEM_RECORD_BYTES as usize];
        system[76] = 1;
        assert!(SystemRecord::decode(&system).is_err());
        system[76] = 0;
        system[72] = 0x80;
        assert!(SystemRecord::decode(&system).is_err());

        let mut station = vec![0; STATION_RECORD_BYTES as usize];
        station[20] = 0x80;
        assert!(StationRecord::decode(&station).is_err());

        let mut commodity = vec![0; COMMODITY_RECORD_BYTES as usize];
        commodity[2] = 1;
        assert!(CommodityCatalogRecord::decode(&commodity).is_err());

        let mut availability = vec![0; AVAILABILITY_RECORD_BYTES as usize];
        availability[12] = 1;
        assert!(AvailabilityRecord::decode(&availability).is_err());

        assert!(MarketRecord::decode(&vec![0; MARKET_RECORD_BYTES as usize - 1]).is_err());
        assert!(
            SymbolCatalogRecord::decode(&vec![0; SYMBOL_CATALOG_RECORD_BYTES as usize - 1])
                .is_err()
        );
    }

    #[test]
    fn every_required_v1_schema_rejects_an_unknown_version() {
        let bytes = include_str!("../fixtures/ebex-v1-full.hex")
            .lines()
            .filter(|line| !line.trim_start().starts_with('#'))
            .flat_map(|line| line.split_whitespace())
            .flat_map(|word| word.as_bytes().chunks_exact(2))
            .map(|pair| u8::from_str_radix(std::str::from_utf8(pair).unwrap(), 16).unwrap())
            .collect::<Vec<_>>();
        let supported = [
            (SECTION_SYSTEMS, SYSTEM_SCHEMA_V1),
            (SECTION_STATIONS, STATION_SCHEMA_V1),
            (SECTION_COMMODITIES, COMMODITY_SCHEMA_V1),
            (SECTION_MARKETS, MARKET_SCHEMA_V1),
            (SECTION_MODULES, MODULE_SCHEMA_V1),
            (SECTION_OUTFITTING, OUTFITTING_SCHEMA_V1),
            (SECTION_SHIPS, SHIP_SCHEMA_V1),
            (SECTION_SHIPYARDS, SHIPYARD_SCHEMA_V1),
        ];
        for directory_index in 0..supported.len() {
            // An unknown schema on a REQUIRED section is a hard reject...
            let mut changed = bytes.clone();
            let entry = HEADER_BYTES + directory_index * DIRECTORY_ENTRY_BYTES;
            put_u16(&mut changed, entry + 2, 2);
            put_u32(&mut changed, entry + 4, 1);
            validate_snapshot(&changed).unwrap();
            assert!(validate_required_sections(&changed, &supported).is_err());
            // ...while the same unknown schema on an optional section is
            // skipped, which is what lets a future section version ship
            // without breaking installed clients.
            let mut optional = bytes.clone();
            put_u16(&mut optional, entry + 2, 2);
            put_u32(&mut optional, entry + 4, 0);
            validate_snapshot(&optional).unwrap();
            validate_required_sections(&optional, &supported).unwrap();
        }
    }

    #[test]
    fn catalogs_and_availability_round_trip() {
        let commodity = CommodityCatalogRecord {
            id: 1,
            symbol_id: 1,
            name_id: 0,
            category_id: 0,
        };
        let module = SymbolCatalogRecord {
            id: 1,
            symbol_id: 1,
        };
        let availability = AvailabilityRecord {
            station_id: 7,
            item_id: 1,
        };
        let mut commodity_bytes = Vec::new();
        commodity.encode_into(&mut commodity_bytes);
        let mut module_bytes = Vec::new();
        module.encode_into(&mut module_bytes);
        let mut availability_bytes = Vec::new();
        availability.encode_into(&mut availability_bytes);
        let mut snapshots = Vec::new();
        snapshots.extend_from_slice(&1_u64.to_le_bytes());
        snapshots.extend_from_slice(&7_u64.to_le_bytes());
        snapshots.extend_from_slice(&9_i64.to_le_bytes());

        let commodity_section = SectionRef {
            id: SECTION_COMMODITIES,
            schema: COMMODITY_SCHEMA_V1,
            required: true,
            record_count: 1,
            record_size: COMMODITY_RECORD_BYTES,
            records: &commodity_bytes,
            auxiliary: &[],
        };
        let module_section = SectionRef {
            id: SECTION_MODULES,
            schema: MODULE_SCHEMA_V1,
            required: true,
            record_count: 1,
            record_size: SYMBOL_CATALOG_RECORD_BYTES,
            records: &module_bytes,
            auxiliary: &[],
        };
        let outfitting_section = SectionRef {
            id: SECTION_OUTFITTING,
            schema: OUTFITTING_SCHEMA_V1,
            required: true,
            record_count: 1,
            record_size: AVAILABILITY_RECORD_BYTES,
            records: &availability_bytes,
            auxiliary: &snapshots,
        };

        assert_eq!(
            commodity_records(commodity_section)
                .unwrap()
                .collect::<Vec<_>>(),
            vec![commodity]
        );
        assert_eq!(
            module_records(module_section).unwrap().collect::<Vec<_>>(),
            vec![module]
        );
        assert_eq!(
            outfitting_records(outfitting_section)
                .unwrap()
                .collect::<Vec<_>>(),
            vec![availability]
        );
        assert_eq!(
            station_snapshots(outfitting_section).unwrap(),
            vec![StationSnapshot {
                station_id: 7,
                observed_at: 9
            }]
        );
    }

    #[test]
    fn section_count_is_derived_from_the_sections_encoded() {
        // `SnapshotHeader` has no section-count field, so a producer cannot
        // claim one section while encoding two; the count is read back.
        let header = SnapshotHeader {
            sequence: 1,
            created_at: 2,
            watermark: 3,
        };
        let section = |id| Section {
            id,
            schema: 1,
            required: false,
            record_count: 0,
            record_size: 0,
            records: Vec::new(),
            auxiliary: Vec::new(),
        };
        let bytes = encode_snapshot(header, vec![section(1), section(2)]).unwrap();
        let metadata = validate_snapshot(&bytes).unwrap();
        assert_eq!(metadata.section_count, 2);
        assert_eq!(metadata.header(), header);
    }

    #[test]
    fn market_baseline_rejects_unsupported_required_schema_before_decoding() {
        let mut bytes = snapshot();
        validate_market_baseline(&bytes, MARKET_BASELINE_SECTIONS).unwrap();
        put_u16(&mut bytes, HEADER_BYTES + 2, MARKET_SCHEMA_V1 + 1);
        let error = validate_market_baseline(&bytes, MARKET_BASELINE_SECTIONS)
            .unwrap_err()
            .to_string();
        assert!(
            error.contains("required EBEX section 5 schema 2"),
            "{error}"
        );
    }

    #[test]
    fn market_baseline_rejects_misordered_rows() {
        let mut records = Vec::new();
        for commodity_id in [1u16, 1u16] {
            MarketRecord {
                station_id: 7,
                commodity_id,
                buy_price: 2,
                sell_price: 3,
                demand: 4,
                supply: 5,
                observed_at: 6,
            }
            .encode_into(&mut records);
        }
        let bytes = encode_snapshot(
            SnapshotHeader {
                sequence: 9,
                created_at: 10,
                watermark: 8,
            },
            vec![Section {
                id: SECTION_MARKETS,
                schema: MARKET_SCHEMA_V1,
                required: true,
                record_count: 2,
                record_size: MARKET_RECORD_BYTES,
                records,
                auxiliary: market_auxiliary_bytes(),
            }],
        )
        .unwrap();
        let error = validate_market_baseline(&bytes, MARKET_BASELINE_SECTIONS)
            .unwrap_err()
            .to_string();
        assert!(error.contains("not strictly sorted"), "{error}");
    }
}
