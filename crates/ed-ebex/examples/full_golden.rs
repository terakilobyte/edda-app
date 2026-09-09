use ed_ebex::*;

fn strings(values: &[&str]) -> Vec<u8> {
    let mut output = Vec::new();
    output.extend_from_slice(&(values.len() as u32).to_le_bytes());
    for (index, value) in values.iter().enumerate() {
        output.extend_from_slice(&((index + 1) as u32).to_le_bytes());
        output.extend_from_slice(&(value.len() as u32).to_le_bytes());
        output.extend_from_slice(value.as_bytes());
    }
    output
}

fn station_snapshots() -> Vec<u8> {
    let mut output = Vec::new();
    output.extend_from_slice(&1_u64.to_le_bytes());
    output.extend_from_slice(&10_u64.to_le_bytes());
    output.extend_from_slice(&150_i64.to_le_bytes());
    output
}

fn section(id: u16, schema: u16, size: u32, records: Vec<u8>, auxiliary: Vec<u8>) -> Section {
    Section {
        id,
        schema,
        // The publication policy: only the markets section is required.
        required: community_section_required(id),
        record_count: records.len() as u64 / u64::from(size),
        record_size: size,
        records,
        auxiliary,
    }
}

fn main() -> anyhow::Result<()> {
    let mut systems = Vec::new();
    SystemRecord {
        address: 10_477_373_803,
        x: 0.0,
        y: 0.0,
        z: 0.0,
        population: 22_781_091_954,
        observed_at: 150,
        name_id: 5,
        security_id: 4,
        allegiance_id: 3,
        controlling_power_id: 1,
        power_state_id: 2,
        powers_id: 1,
        flags: SystemRecord::HAS_COORDINATES | SystemRecord::HAS_POPULATION,
    }
    .encode_into(&mut systems);

    let mut stations = Vec::new();
    StationRecord {
        id: 10,
        system_address: 10_477_373_803,
        name_id: 1,
        flags: StationRecord::HAS_MARKET
            | StationRecord::HAS_OUTFITTING
            | StationRecord::HAS_SHIPYARD,
        market_observed_at: 150,
        outfitting_observed_at: 150,
        shipyard_observed_at: 150,
    }
    .encode_into(&mut stations);

    let mut commodities = Vec::new();
    CommodityCatalogRecord {
        id: 1,
        symbol_id: 3,
        name_id: 1,
        category_id: 2,
    }
    .encode_into(&mut commodities);
    let mut market = Vec::new();
    MarketRecord {
        station_id: 10,
        commodity_id: 1,
        buy_price: 50_000,
        sell_price: 60_000,
        demand: 20,
        supply: 30,
        observed_at: 150,
    }
    .encode_into(&mut market);
    let mut market_auxiliary = Vec::new();
    market_auxiliary.extend_from_slice(&1_u16.to_le_bytes());
    market_auxiliary.extend_from_slice(&1_u16.to_le_bytes());
    market_auxiliary.extend_from_slice(&4_u16.to_le_bytes());
    market_auxiliary.extend_from_slice(&4_u16.to_le_bytes());
    market_auxiliary.extend_from_slice(&6_u16.to_le_bytes());
    market_auxiliary.extend_from_slice(b"goldGoldMetals");
    market_auxiliary.extend_from_slice(&station_snapshots());

    let mut modules = Vec::new();
    SymbolCatalogRecord {
        id: 1,
        symbol_id: 1,
    }
    .encode_into(&mut modules);
    let mut ships = Vec::new();
    SymbolCatalogRecord {
        id: 1,
        symbol_id: 1,
    }
    .encode_into(&mut ships);
    let mut outfitting = Vec::new();
    AvailabilityRecord {
        station_id: 10,
        item_id: 1,
    }
    .encode_into(&mut outfitting);
    let mut shipyards = Vec::new();
    AvailabilityRecord {
        station_id: 10,
        item_id: 1,
    }
    .encode_into(&mut shipyards);

    // Stars: a provisional (negative) address sorts first, signed.
    let mut stars = Vec::new();
    StarRecord { address: -42, class: 2, scoopable: true, observed_at: 150 }.encode_into(&mut stars);
    StarRecord { address: 10_477_373_803, class: 14, scoopable: false, observed_at: 150 }.encode_into(&mut stars);

    let bytes = encode_snapshot(
        SnapshotHeader {
            sequence: 100,
            created_at: 200,
            watermark: 150,
        },
        vec![
            section(
                SECTION_SYSTEMS,
                SYSTEM_SCHEMA_V1,
                SYSTEM_RECORD_BYTES,
                systems,
                strings(&["Aisling Duval", "Control", "Federation", "High", "Sol"]),
            ),
            section(
                SECTION_STATIONS,
                STATION_SCHEMA_V1,
                STATION_RECORD_BYTES,
                stations,
                strings(&["Galileo"]),
            ),
            section(
                SECTION_COMMODITIES,
                COMMODITY_SCHEMA_V1,
                COMMODITY_RECORD_BYTES,
                commodities,
                strings(&["Gold", "Metals", "gold"]),
            ),
            section(
                SECTION_MARKETS,
                MARKET_SCHEMA_V1,
                MARKET_RECORD_BYTES,
                market,
                market_auxiliary,
            ),
            section(
                SECTION_MODULES,
                MODULE_SCHEMA_V1,
                SYMBOL_CATALOG_RECORD_BYTES,
                modules,
                strings(&["int_hyperdrive_size2_class1"]),
            ),
            section(
                SECTION_OUTFITTING,
                OUTFITTING_SCHEMA_V1,
                AVAILABILITY_RECORD_BYTES,
                outfitting,
                station_snapshots(),
            ),
            section(
                SECTION_SHIPS,
                SHIP_SCHEMA_V1,
                SYMBOL_CATALOG_RECORD_BYTES,
                ships,
                strings(&["cobramkiii"]),
            ),
            section(
                SECTION_SHIPYARDS,
                SHIPYARD_SCHEMA_V1,
                AVAILABILITY_RECORD_BYTES,
                shipyards,
                station_snapshots(),
            ),
            section(SECTION_STARS, STAR_SCHEMA_V1, STAR_RECORD_BYTES, stars, vec![]),
        ],
    )?;
    for chunk in bytes.chunks(32) {
        for byte in chunk {
            print!("{byte:02x}");
        }
        println!();
    }
    Ok(())
}
