use std::{env, fs};

use anyhow::{Context, Result};

fn main() -> Result<()> {
    let path = env::args()
        .nth(1)
        .context("usage: inspect <artifact.ebex.zst>")?;
    let compressed = fs::read(&path).with_context(|| format!("reading {path}"))?;
    let bytes = ed_ebex::decompress(&compressed)?;
    let metadata = ed_ebex::validate_snapshot(&bytes)?;
    println!(
        "sequence={} created_at={} watermark={} sections={} compressed_bytes={} uncompressed_bytes={}",
        metadata.sequence,
        metadata.created_at,
        metadata.watermark,
        metadata.section_count,
        compressed.len(),
        bytes.len()
    );
    for section in ed_ebex::sections_prevalidated(&bytes)? {
        println!(
            "section={} schema={} required={} records={} record_bytes={} auxiliary_bytes={}",
            section.id,
            section.schema,
            section.required,
            section.record_count,
            section.records.len(),
            section.auxiliary.len()
        );
        if section.id == ed_ebex::SECTION_MARKETS {
            let auxiliary = ed_ebex::market_auxiliary(section)?;
            println!(
                "market_commodities={} market_station_snapshots={}",
                auxiliary.commodities.len(),
                auxiliary.stations.len()
            );
        } else if section.id == ed_ebex::SECTION_SYSTEMS {
            println!(
                "systems={} system_strings={}",
                ed_ebex::system_records(section)?.count(),
                ed_ebex::string_table(section)?.len()
            );
        } else if section.id == ed_ebex::SECTION_STATIONS {
            println!(
                "stations={} station_strings={}",
                ed_ebex::station_records(section)?.count(),
                ed_ebex::string_table(section)?.len()
            );
        } else if section.id == ed_ebex::SECTION_COMMODITIES {
            println!(
                "commodities={} commodity_strings={}",
                ed_ebex::commodity_records(section)?.count(),
                ed_ebex::string_table(section)?.len()
            );
        } else if section.id == ed_ebex::SECTION_MODULES {
            println!(
                "modules={} module_strings={}",
                ed_ebex::module_records(section)?.count(),
                ed_ebex::string_table(section)?.len()
            );
        } else if section.id == ed_ebex::SECTION_OUTFITTING {
            println!(
                "outfitting={} outfitting_station_snapshots={}",
                ed_ebex::outfitting_records(section)?.count(),
                ed_ebex::station_snapshots(section)?.len()
            );
        } else if section.id == ed_ebex::SECTION_SHIPS {
            println!(
                "ships={} ship_strings={}",
                ed_ebex::ship_records(section)?.count(),
                ed_ebex::string_table(section)?.len()
            );
        } else if section.id == ed_ebex::SECTION_SHIPYARDS {
            println!(
                "shipyard={} shipyard_station_snapshots={}",
                ed_ebex::shipyard_records(section)?.count(),
                ed_ebex::station_snapshots(section)?.len()
            );
        }
    }
    Ok(())
}
