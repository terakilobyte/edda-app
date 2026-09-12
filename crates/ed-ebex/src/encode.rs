//! Encoders for the auxiliary regions whose decoders live in the crate
//! root — the market dictionary, station-snapshot directories, string
//! tables, symbol catalogs and availability records — plus the canonical
//! section plans for each published product. Both halves of every byte
//! layout live in this crate, round-trip tested together.

use std::collections::BTreeMap;

use anyhow::{Context, Result};

use crate::{
    AvailabilityRecord, SectionPlan, SymbolCatalogRecord, AVAILABILITY_RECORD_BYTES,
    COMMODITY_RECORD_BYTES, COMMODITY_SCHEMA_V1, MARKET_RECORD_BYTES, MARKET_SCHEMA_V1,
    MODULE_SCHEMA_V1, OUTFITTING_SCHEMA_V1, PROHIBITED_RECORD_BYTES, PROHIBITED_SCHEMA_V1,
    SECTION_COMMODITIES, SECTION_MARKETS, SECTION_MODULES, SECTION_OUTFITTING, SECTION_PROHIBITED,
    SECTION_SHIPS, SECTION_SHIPYARDS, SECTION_STARS, SECTION_STATIONS, SECTION_STATION_DETAILS,
    SECTION_SYSTEMS, SHIPYARD_SCHEMA_V1, SHIP_SCHEMA_V1, STAR_RECORD_BYTES, STAR_SCHEMA_V1,
    STATION_DETAILS_RECORD_BYTES, STATION_DETAILS_SCHEMA_V1, STATION_RECORD_BYTES,
    STATION_SCHEMA_V1, SYMBOL_CATALOG_RECORD_BYTES, SYSTEM_RECORD_BYTES, SYSTEM_SCHEMA_V1,
};

/// Whether a community-baseline section is published with the required
/// bit. Only the markets section: released clients hydrate markets and
/// skip the rest, so marking anything else required would brick every
/// installed client the day it ships. A section becomes required only
/// when no supported client would be broken by it.
pub fn community_section_required(id: u16) -> bool {
    id == SECTION_MARKETS
}

/// The community baseline's sections, in the order the writer opens them:
/// the one canonical (id, schema, required, record size) table both the
/// publisher's plan and the validators derive from.
pub fn community_section_plans() -> [SectionPlan; 10] {
    [
        SectionPlan {
            id: SECTION_SYSTEMS,
            schema: SYSTEM_SCHEMA_V1,
            required: community_section_required(SECTION_SYSTEMS),
            record_size: SYSTEM_RECORD_BYTES,
        },
        SectionPlan {
            id: SECTION_STATIONS,
            schema: STATION_SCHEMA_V1,
            required: community_section_required(SECTION_STATIONS),
            record_size: STATION_RECORD_BYTES,
        },
        SectionPlan {
            id: SECTION_COMMODITIES,
            schema: COMMODITY_SCHEMA_V1,
            required: community_section_required(SECTION_COMMODITIES),
            record_size: COMMODITY_RECORD_BYTES,
        },
        SectionPlan {
            id: SECTION_MARKETS,
            schema: MARKET_SCHEMA_V1,
            required: community_section_required(SECTION_MARKETS),
            record_size: MARKET_RECORD_BYTES,
        },
        SectionPlan {
            id: SECTION_MODULES,
            schema: MODULE_SCHEMA_V1,
            required: community_section_required(SECTION_MODULES),
            record_size: SYMBOL_CATALOG_RECORD_BYTES,
        },
        SectionPlan {
            id: SECTION_OUTFITTING,
            schema: OUTFITTING_SCHEMA_V1,
            required: community_section_required(SECTION_OUTFITTING),
            record_size: AVAILABILITY_RECORD_BYTES,
        },
        SectionPlan {
            id: SECTION_SHIPS,
            schema: SHIP_SCHEMA_V1,
            required: community_section_required(SECTION_SHIPS),
            record_size: SYMBOL_CATALOG_RECORD_BYTES,
        },
        SectionPlan {
            id: SECTION_SHIPYARDS,
            schema: SHIPYARD_SCHEMA_V1,
            required: community_section_required(SECTION_SHIPYARDS),
            record_size: AVAILABILITY_RECORD_BYTES,
        },
        // 2026-09-04 addendum, both OPTIONAL: old clients skip them, and
        // the sections may legitimately be thin while the Docked-event
        // ingest is young (lean now, fatter as the data grows).
        SectionPlan {
            id: SECTION_STATION_DETAILS,
            schema: STATION_DETAILS_SCHEMA_V1,
            required: false,
            record_size: STATION_DETAILS_RECORD_BYTES,
        },
        SectionPlan {
            id: SECTION_PROHIBITED,
            schema: PROHIBITED_SCHEMA_V1,
            required: false,
            record_size: PROHIBITED_RECORD_BYTES,
        },
    ]
}

/// The stars product's single section. Optional: the product is merged
/// into star overrides by clients that know it and skipped by ones that
/// do not, and nothing else in the artifact depends on it.
pub fn stars_section_plan() -> SectionPlan {
    SectionPlan {
        id: SECTION_STARS,
        schema: STAR_SCHEMA_V1,
        required: false,
        record_size: STAR_RECORD_BYTES,
    }
}

fn checked_u64(value: i64, field: &str) -> Result<u64> {
    u64::try_from(value).with_context(|| format!("{field} is outside EBEX u64 range: {value}"))
}

/// Encode the market auxiliary region: the commodity dictionary (ids are
/// 1-based positions in `commodities`, each entry `(symbol, display
/// name, category)` — empty name/category encode as length 0) followed
/// by the station-snapshot directory. Decoded by
/// [`crate::market_auxiliary`].
pub fn encode_market_auxiliary(
    commodities: &[(String, String, String)],
    stations: &[(i64, i64)],
) -> Result<Vec<u8>> {
    let mut output = Vec::new();
    output.extend_from_slice(&u16::try_from(commodities.len())?.to_le_bytes());
    for (index, (symbol, name, category)) in commodities.iter().enumerate() {
        let (symbol, name, category) = (symbol.as_bytes(), name.as_bytes(), category.as_bytes());
        output.extend_from_slice(&u16::try_from(index + 1)?.to_le_bytes());
        output.extend_from_slice(&u16::try_from(symbol.len())?.to_le_bytes());
        output.extend_from_slice(&u16::try_from(name.len())?.to_le_bytes());
        output.extend_from_slice(&u16::try_from(category.len())?.to_le_bytes());
        output.extend_from_slice(symbol);
        output.extend_from_slice(name);
        output.extend_from_slice(category);
    }
    output.extend_from_slice(&encode_station_snapshots(stations)?);
    Ok(output)
}

/// Encode a station-snapshot directory: count, then `(station id,
/// observed_at)` pairs. Decoded by [`crate::station_snapshots`] and as
/// the tail of [`crate::market_auxiliary`].
pub fn encode_station_snapshots(stations: &[(i64, i64)]) -> Result<Vec<u8>> {
    let mut output = Vec::with_capacity(8 + stations.len().saturating_mul(16));
    output.extend_from_slice(&u64::try_from(stations.len())?.to_le_bytes());
    for (station_id, observed_at) in stations {
        output.extend_from_slice(&checked_u64(*station_id, "station id")?.to_le_bytes());
        output.extend_from_slice(&observed_at.to_le_bytes());
    }
    Ok(output)
}

/// Build a string table from `values`: empty strings are dropped, the
/// rest are deduplicated and sorted, ids are 1-based positions. Returns
/// the value → id map and the encoded table. Decoded by
/// [`crate::string_table`].
pub fn build_string_table<'a>(
    values: impl Iterator<Item = &'a str>,
) -> Result<(BTreeMap<String, u32>, Vec<u8>)> {
    let values = values
        .filter(|value| !value.is_empty())
        .map(str::to_owned)
        .collect::<std::collections::BTreeSet<_>>();
    let mut ids = BTreeMap::new();
    let mut bytes = Vec::new();
    bytes.extend_from_slice(&u32::try_from(values.len())?.to_le_bytes());
    for (index, value) in values.into_iter().enumerate() {
        let id = u32::try_from(index + 1)?;
        bytes.extend_from_slice(&id.to_le_bytes());
        bytes.extend_from_slice(&u32::try_from(value.len())?.to_le_bytes());
        bytes.extend_from_slice(value.as_bytes());
        ids.insert(value, id);
    }
    Ok((ids, bytes))
}

/// Resolve an optional string against a [`build_string_table`] map;
/// id 0 means absent.
pub fn string_id(ids: &BTreeMap<String, u32>, value: Option<&String>) -> Result<u32> {
    match value {
        Some(value) if !value.is_empty() => ids
            .get(value)
            .copied()
            .context("identity string missing from EBEX string table"),
        _ => Ok(0),
    }
}

/// A symbol catalog ready to write: the record bytes, their string-table
/// auxiliary, and the symbol → catalog-id map availability sections
/// reference.
pub struct EncodedSymbolCatalog {
    pub ids: BTreeMap<String, u32>,
    pub records: Vec<u8>,
    pub strings: Vec<u8>,
}

/// Encode a symbol catalog (modules or ships): catalog ids are 1-based
/// positions in `symbols`. Decoded by [`crate::module_records`] /
/// [`crate::ship_records`] with [`crate::string_table`].
pub fn encode_symbol_catalog(symbols: &[String]) -> Result<EncodedSymbolCatalog> {
    let (string_ids, strings) = build_string_table(symbols.iter().map(String::as_str))?;
    let mut records = Vec::with_capacity(
        symbols
            .len()
            .saturating_mul(SYMBOL_CATALOG_RECORD_BYTES as usize),
    );
    let mut ids = BTreeMap::new();
    for (index, symbol) in symbols.iter().enumerate() {
        let id = u32::try_from(index + 1)?;
        SymbolCatalogRecord {
            id,
            symbol_id: string_id(&string_ids, Some(symbol))?,
        }
        .encode_into(&mut records);
        ids.insert(symbol.clone(), id);
    }
    Ok(EncodedSymbolCatalog {
        ids,
        records,
        strings,
    })
}

/// Encode an availability section's records (outfitting or shipyard),
/// sorted by `(station id, catalog id)`. Decoded by
/// [`crate::outfitting_records`] / [`crate::shipyard_records`].
pub fn encode_availability(
    rows: &[(i64, String)],
    item_ids: &BTreeMap<String, u32>,
) -> Result<Vec<u8>> {
    let mut records = Vec::with_capacity(
        rows.len()
            .saturating_mul(AVAILABILITY_RECORD_BYTES as usize),
    );
    let mut resolved = rows
        .iter()
        .map(|(station_id, symbol)| {
            Ok((
                checked_u64(*station_id, "station id")?,
                *item_ids
                    .get(symbol)
                    .context("availability item missing from catalog")?,
            ))
        })
        .collect::<Result<Vec<_>>>()?;
    resolved.sort_unstable();
    for (station_id, item_id) in resolved {
        AvailabilityRecord {
            station_id,
            item_id,
        }
        .encode_into(&mut records);
    }
    Ok(records)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{
        encode_snapshot, market_auxiliary, section, ship_records, string_table, validate_snapshot,
        Section, SnapshotHeader, ALL_V1_SECTIONS,
    };

    /// The canonical plan table and `ALL_V1_SECTIONS` describe the same
    /// eight sections — by construction of this test, not by discipline.
    #[test]
    fn community_plans_match_the_supported_section_table() {
        let plans = community_section_plans();
        assert_eq!(
            plans.iter().map(|p| (p.id, p.schema)).collect::<Vec<_>>(),
            ALL_V1_SECTIONS.to_vec()
        );
        assert_eq!(
            plans
                .iter()
                .filter(|p| p.required)
                .map(|p| p.id)
                .collect::<Vec<_>>(),
            vec![SECTION_MARKETS],
            "markets is the one required community section"
        );
        assert!(!stars_section_plan().required);
    }

    /// Encoder and decoder of the market auxiliary region agree — display
    /// names and categories included, absent ones as empty strings.
    #[test]
    fn market_auxiliary_round_trips() {
        let auxiliary = encode_market_auxiliary(
            &[
                ("gold".into(), "Gold".into(), "Metals".into()),
                ("silver".into(), String::new(), String::new()),
            ],
            &[(7, 80), (9, 81)],
        )
        .unwrap();
        let bytes = encode_snapshot(
            SnapshotHeader {
                sequence: 1,
                created_at: 90,
                watermark: 81,
            },
            vec![Section {
                id: SECTION_MARKETS,
                schema: MARKET_SCHEMA_V1,
                required: true,
                record_count: 0,
                record_size: MARKET_RECORD_BYTES,
                records: vec![],
                auxiliary,
            }],
        )
        .unwrap();
        validate_snapshot(&bytes).unwrap();
        let markets = section(&bytes, SECTION_MARKETS).unwrap().unwrap();
        let decoded = market_auxiliary(markets).unwrap();
        assert_eq!(
            decoded
                .commodities
                .iter()
                .map(|c| (
                    c.id,
                    c.symbol.as_str(),
                    c.name.as_str(),
                    c.category.as_str()
                ))
                .collect::<Vec<_>>(),
            vec![(1, "gold", "Gold", "Metals"), (2, "silver", "", "")]
        );
        assert_eq!(
            decoded
                .stations
                .iter()
                .map(|s| (s.station_id, s.observed_at))
                .collect::<Vec<_>>(),
            vec![(7, 80), (9, 81)]
        );
    }

    /// Encoder and decoder of a symbol catalog with its string table agree.
    #[test]
    fn symbol_catalog_round_trips() {
        let catalog = encode_symbol_catalog(&["adder".into(), "anaconda".into()]).unwrap();
        let bytes = encode_snapshot(
            SnapshotHeader {
                sequence: 1,
                created_at: 1,
                watermark: 1,
            },
            vec![Section {
                id: SECTION_SHIPS,
                schema: SHIP_SCHEMA_V1,
                required: false,
                record_count: 2,
                record_size: SYMBOL_CATALOG_RECORD_BYTES,
                records: catalog.records.clone(),
                auxiliary: catalog.strings.clone(),
            }],
        )
        .unwrap();
        let ships = section(&bytes, SECTION_SHIPS).unwrap().unwrap();
        let strings = string_table(ships).unwrap();
        let by_id: BTreeMap<u32, &str> = strings.iter().map(|s| (s.id, s.value.as_str())).collect();
        let decoded: Vec<(u32, &str)> = ship_records(ships)
            .unwrap()
            .map(|record| (record.id, by_id[&record.symbol_id]))
            .collect();
        assert_eq!(decoded, vec![(1, "adder"), (2, "anaconda")]);
        assert_eq!(catalog.ids["anaconda"], 2);
    }

    /// Availability records come out sorted regardless of input order, and
    /// a symbol missing from the catalog is an error, not a zero id.
    #[test]
    fn availability_encodes_sorted_and_checked() {
        let catalog = encode_symbol_catalog(&["beam".into(), "pulse".into()]).unwrap();
        let records =
            encode_availability(&[(9, "pulse".into()), (7, "beam".into())], &catalog.ids).unwrap();
        let decoded: Vec<AvailabilityRecord> = records
            .chunks_exact(AVAILABILITY_RECORD_BYTES as usize)
            .map(|chunk| AvailabilityRecord::decode(chunk).unwrap())
            .collect();
        assert_eq!(
            decoded[0],
            AvailabilityRecord {
                station_id: 7,
                item_id: catalog.ids["beam"]
            }
        );
        assert_eq!(
            decoded[1],
            AvailabilityRecord {
                station_id: 9,
                item_id: catalog.ids["pulse"]
            }
        );
        assert!(encode_availability(&[(7, "missing".into())], &catalog.ids).is_err());
    }

    /// String ids resolve, absent strings are id 0, unknown strings error.
    #[test]
    fn string_table_round_trips_with_id_resolution() {
        let (ids, bytes) = build_string_table(["b", "", "a", "b"].into_iter()).unwrap();
        assert_eq!(ids.len(), 2);
        assert_eq!(string_id(&ids, Some(&"a".to_string())).unwrap(), ids["a"]);
        assert_eq!(string_id(&ids, None).unwrap(), 0);
        assert_eq!(string_id(&ids, Some(&String::new())).unwrap(), 0);
        assert!(string_id(&ids, Some(&"zzz".to_string())).is_err());
        // Decode through the reader by riding as a catalog auxiliary.
        let catalog = encode_symbol_catalog(&["a".into(), "b".into()]).unwrap();
        assert_eq!(catalog.strings, bytes, "same table, same bytes");
    }
}
