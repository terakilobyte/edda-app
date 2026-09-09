use anyhow::{Context, Result};
use rusqlite::{params, Connection, OpenFlags};
use std::{collections::HashMap, env, fs, io::Cursor, path::Path, time::Instant};

const RECORD_BYTES: usize = 34;

#[derive(Clone)]
struct Commodity {
    id: u16,
    symbol: String,
    name: String,
    category: String,
}

fn put_record(out: &mut Vec<u8>, values: [u64; 7]) {
    out.extend_from_slice(&values[0].to_le_bytes());
    out.extend_from_slice(&(values[1] as u16).to_le_bytes());
    for value in &values[2..6] {
        out.extend_from_slice(&(*value as u32).to_le_bytes());
    }
    out.extend_from_slice(&(values[6] as i64).to_le_bytes());
}

fn u16_at(data: &[u8], at: usize) -> u16 {
    u16::from_le_bytes(data[at..at + 2].try_into().unwrap())
}
fn u32_at(data: &[u8], at: usize) -> u32 {
    u32::from_le_bytes(data[at..at + 4].try_into().unwrap())
}
fn u64_at(data: &[u8], at: usize) -> u64 {
    u64::from_le_bytes(data[at..at + 8].try_into().unwrap())
}
fn i64_at(data: &[u8], at: usize) -> i64 {
    i64::from_le_bytes(data[at..at + 8].try_into().unwrap())
}

fn create_snapshot(path: &Path) -> Result<Connection> {
    if path.exists() { fs::remove_file(path)?; }
    let db = Connection::open(path)?;
    db.execute_batch(
        "PRAGMA journal_mode=OFF; PRAGMA synchronous=OFF; PRAGMA locking_mode=EXCLUSIVE;
         CREATE TABLE commodities(id INTEGER PRIMARY KEY, symbol TEXT NOT NULL UNIQUE, name TEXT, category TEXT);
         CREATE TABLE market(station_id INTEGER NOT NULL, commodity_id INTEGER NOT NULL,
           buy_price INTEGER, sell_price INTEGER, demand INTEGER, supply INTEGER, updated INTEGER,
           PRIMARY KEY(station_id, commodity_id)) WITHOUT ROWID;"
    )?;
    Ok(db)
}

fn main() -> Result<()> {
    let mut args = env::args().skip(1);
    let source_path = args.next().context("usage: bench DATABASE [ROWS]")?;
    let limit: i64 = args.next().as_deref().unwrap_or("5000000").parse()?;
    let out_setting = env::var_os("EDDA_BENCH_OUT").unwrap_or_else(|| ".bench-market/out-rust".into());
    let out_dir = Path::new(&out_setting);
    fs::create_dir_all(out_dir)?;
    let sqlite_path = out_dir.join("market.sqlite3");
    let hydrated_path = out_dir.join("hydrated.sqlite3");

    let source = Connection::open_with_flags(source_path, OpenFlags::SQLITE_OPEN_READ_ONLY)?;
    let mut snapshot = create_snapshot(&sqlite_path)?;
    let started = Instant::now();
    let tx = snapshot.transaction()?;
    let mut insert_market = tx.prepare_cached("INSERT INTO market VALUES(?,?,?,?,?,?,?)")?;
    let mut query = source.prepare(
        "SELECT station_id,symbol,coalesce(name,''),coalesce(category,''),
         coalesce(buy_price,0),coalesce(sell_price,0),coalesce(demand,0),coalesce(supply,0),
         coalesce(CAST(strftime('%s',updated) AS INTEGER),0) FROM sys_market LIMIT ?"
    )?;
    let mut rows = query.query([limit])?;
    let mut symbols: HashMap<String, u16> = HashMap::new();
    let mut commodities = Vec::new();
    let mut records = Vec::with_capacity(limit as usize * RECORD_BYTES + 16);
    records.extend_from_slice(b"EDMK\x01\0\0\0");
    records.extend_from_slice(&0u64.to_le_bytes());
    let mut count = 0u64;
    while let Some(row) = rows.next()? {
        let symbol: String = row.get(1)?;
        let commodity = match symbols.get(&symbol) {
            Some(id) => *id,
            None => {
                let id = u16::try_from(symbols.len() + 1)?;
                commodities.push(Commodity { id, symbol: symbol.clone(), name: row.get(2)?, category: row.get(3)? });
                symbols.insert(symbol, id);
                id
            }
        };
        let values = [row.get::<_, i64>(0)? as u64, commodity as u64, row.get::<_, i64>(4)? as u64,
            row.get::<_, i64>(5)? as u64, row.get::<_, i64>(6)? as u64, row.get::<_, i64>(7)? as u64,
            row.get::<_, i64>(8)? as u64];
        put_record(&mut records, values);
        insert_market.execute(params![values[0] as i64, commodity, values[2] as i64, values[3] as i64, values[4] as i64, values[5] as i64, values[6] as i64])?;
        count += 1;
    }
    drop(insert_market);
    {
        let mut insert = tx.prepare_cached("INSERT INTO commodities VALUES(?,?,?,?)")?;
        for item in &commodities { insert.execute(params![item.id, item.symbol, item.name, item.category])?; }
    }
    tx.commit()?;
    records[8..16].copy_from_slice(&count.to_le_bytes());
    records.extend_from_slice(&(commodities.len() as u16).to_le_bytes());
    for item in &commodities {
        let fields = [item.symbol.as_bytes(), item.name.as_bytes(), item.category.as_bytes()];
        records.extend_from_slice(&item.id.to_le_bytes());
        for field in fields { records.extend_from_slice(&(field.len() as u16).to_le_bytes()); }
        for field in fields { records.extend_from_slice(field); }
    }
    let build_time = started.elapsed();
    fs::write(out_dir.join("market.bin"), &records)?;

    let started = Instant::now();
    let sqlite_zstd = zstd::stream::encode_all(Cursor::new(fs::read(&sqlite_path)?), 9)?;
    let sqlite_compress = started.elapsed();
    fs::write(out_dir.join("market.sqlite3.zst"), &sqlite_zstd)?;
    let started = Instant::now();
    let binary_zstd = zstd::stream::encode_all(Cursor::new(&records), 9)?;
    let binary_compress = started.elapsed();
    fs::write(out_dir.join("market.bin.zst"), &binary_zstd)?;

    let started = Instant::now();
    let decoded = zstd::stream::decode_all(Cursor::new(&binary_zstd))?;
    let decoded_count = u64_at(&decoded, 8) as usize;
    let mut checksum = 0u64;
    for i in 0..decoded_count {
        let at = 16 + i * RECORD_BYTES;
        checksum ^= u64_at(&decoded, at) ^ u16_at(&decoded, at + 8) as u64 ^ u32_at(&decoded, at + 14) as u64;
    }
    let decode_time = started.elapsed();

    let started = Instant::now();
    let mut hydrated = create_snapshot(&hydrated_path)?;
    let tx = hydrated.transaction()?;
    {
        let mut at = 16 + decoded_count * RECORD_BYTES;
        let dictionary_count = u16_at(&decoded, at) as usize;
        at += 2;
        let mut insert = tx.prepare_cached("INSERT INTO commodities VALUES(?,?,?,?)")?;
        for _ in 0..dictionary_count {
            let id = u16_at(&decoded, at);
            let lengths = [u16_at(&decoded, at + 2) as usize, u16_at(&decoded, at + 4) as usize, u16_at(&decoded, at + 6) as usize];
            at += 8;
            let mut fields = Vec::with_capacity(3);
            for length in lengths {
                fields.push(std::str::from_utf8(&decoded[at..at + length])?);
                at += length;
            }
            insert.execute(params![id, fields[0], fields[1], fields[2]])?;
        }
    }
    {
        let mut insert = tx.prepare_cached("INSERT INTO market VALUES(?,?,?,?,?,?,?)")?;
        for i in 0..decoded_count {
            let at = 16 + i * RECORD_BYTES;
            insert.execute(params![u64_at(&decoded, at) as i64, u16_at(&decoded, at+8), u32_at(&decoded, at+10),
                u32_at(&decoded, at+14), u32_at(&decoded, at+18), u32_at(&decoded, at+22), i64_at(&decoded, at+26)])?;
        }
    }
    tx.commit()?;
    let hydrate_time = started.elapsed();
    let hydrated_rows: i64 = hydrated.query_row("SELECT count(*) FROM market", [], |row| row.get(0))?;
    let hydrated_commodities: i64 = hydrated.query_row("SELECT count(*) FROM commodities", [], |row| row.get(0))?;

    println!("rows={count} commodities={} record_bytes={RECORD_BYTES}", commodities.len());
    println!("build_sqlite_and_binary_seconds={:.3}", build_time.as_secs_f64());
    println!("sqlite_bytes={} sqlite_zstd_bytes={} compress_seconds={:.3}", fs::metadata(&sqlite_path)?.len(), sqlite_zstd.len(), sqlite_compress.as_secs_f64());
    println!("binary_bytes={} binary_zstd_bytes={} compress_seconds={:.3}", records.len(), binary_zstd.len(), binary_compress.as_secs_f64());
    println!("binary_decode_seconds={:.3} checksum={checksum}", decode_time.as_secs_f64());
    println!("binary_to_sqlite_hydrate_seconds={:.3} hydrated_bytes={} hydrated_rows={hydrated_rows} hydrated_commodities={hydrated_commodities}", hydrate_time.as_secs_f64(), fs::metadata(&hydrated_path)?.len());
    Ok(())
}
