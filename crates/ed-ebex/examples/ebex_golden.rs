use ed_ebex::{
    encode_snapshot, MarketRecord, Section, SnapshotHeader, MARKET_RECORD_BYTES, MARKET_SCHEMA_V1,
    SECTION_MARKETS,
};

fn main() -> anyhow::Result<()> {
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
            record_count: 1,
            record_size: MARKET_RECORD_BYTES,
            records,
            auxiliary,
        }],
    )?;

    for chunk in bytes.chunks(16) {
        for byte in chunk {
            print!("{byte:02x}");
        }
        println!();
    }
    Ok(())
}
