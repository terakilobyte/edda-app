//! Cut a fixture-sized slice out of a full `galaxy.sqlite3`: every system
//! within `radius_ly` of `origin`, plus everything hanging off those systems
//! (stations, markets, outfitting, shipyards, services, factions, bodies,
//! rings, hotspots, star overrides) and the whole of the small catalog
//! tables (commodities, modules, ships, meta).
//!
//!     cargo run -p ed-store --example extract_bubble --release -- \
//!         .data/galaxy.sqlite3 Wongi 200 fixtures/galaxy-wongi-200.sqlite3
//!
//! The source is opened read-only. The output is created through
//! `schema::attach_galaxy`, so it carries the current DDL and user_version
//! and can be attached by the app or tests exactly like the real file.

use std::path::PathBuf;

use anyhow::{bail, Context, Result};
use rusqlite::Connection;

fn main() -> Result<()> {
    let args: Vec<String> = std::env::args().skip(1).collect();
    if args.len() != 4 {
        eprintln!("usage: extract_bubble <galaxy.sqlite3> <origin system> <radius_ly> <out.sqlite3>");
        std::process::exit(2);
    }
    let src = PathBuf::from(&args[0]);
    let origin = &args[1];
    let radius: f64 = args[2].parse().context("radius_ly must be a number")?;
    let out = PathBuf::from(&args[3]);
    if out.exists() {
        bail!("{} already exists; remove it first", out.display());
    }
    if let Some(parent) = out.parent() {
        std::fs::create_dir_all(parent)?;
    }

    let conn = Connection::open_in_memory()?;
    ed_store::schema::attach_galaxy(&conn, Some(&out)).context("creating output galaxy")?;
    conn.execute(
        "ATTACH DATABASE ?1 AS src",
        [format!("file:{}?mode=ro", src.to_string_lossy())],
    )
    .context("attaching source read-only")?;

    let (ox, oy, oz): (f64, f64, f64) = conn
        .query_row(
            "SELECT x, y, z FROM src.sys_systems WHERE name = ?1 COLLATE NOCASE",
            [origin],
            |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)),
        )
        .with_context(|| format!("origin system {origin:?} not found in source"))?;
    eprintln!("origin {origin} at ({ox:.2}, {oy:.2}, {oz:.2}), radius {radius} ly");

    let started = std::time::Instant::now();
    conn.execute_batch("BEGIN")?;

    // Systems by bounding box on the indexed x column, then exact distance.
    // Bound parameters, not interpolation: a negative coordinate written as
    // `y--12.28` would start a SQL line comment and swallow the statement.
    let list = quoted(&common_columns(&conn, "sys_systems")?);
    let n = conn
        .execute(
            &format!(
                "INSERT OR REPLACE INTO galaxy.sys_systems ({list}) SELECT {list} FROM src.sys_systems
                 WHERE x BETWEEN ?1 - ?4 AND ?1 + ?4
                   AND ((x-?1)*(x-?1) + (y-?2)*(y-?2) + (z-?3)*(z-?3)) <= ?4 * ?4"
            ),
            rusqlite::params![ox, oy, oz, radius],
        )
        .context("copying sys_systems")?;
    eprintln!("{:>22}: {n} rows", "sys_systems");
    let by_system = "system_id64 IN (SELECT id64 FROM galaxy.sys_systems)";
    let by_station = "station_id IN (SELECT id FROM galaxy.sys_stations)";
    let by_body = "body_id64 IN (SELECT id64 FROM galaxy.sys_bodies)";
    for (table, filter) in [
        ("sys_stations", by_system),
        ("sys_factions", by_system),
        ("sys_bodies", by_system),
        ("sys_market", by_station),
        ("sys_outfitting", by_station),
        ("sys_shipyard", by_station),
        ("sys_station_services", by_station),
        ("sys_market_prohibited", by_station),
        ("sys_body_materials", by_body),
        ("sys_rings", by_body),
        ("sys_ring_hotspots", by_body),
        ("star_overrides", "id64 IN (SELECT id64 FROM galaxy.sys_systems)"),
        ("sys_commodities", "1"),
        ("sys_modules", "1"),
        ("sys_ships", "1"),
        ("sys_meta", "1"),
    ] {
        if table_exists(&conn, "src", table)? {
            copy(&conn, table, filter)?;
        } else if legacy_catalog(table) {
            let n: i64 = conn.query_row(&format!("SELECT COUNT(*) FROM galaxy.{table}"), [], |r| r.get(0))?;
            eprintln!("{table:>22}: {n} rows (built from legacy symbols)");
        } else {
            eprintln!("{table:>22}: absent in source, skipped");
        }
    }
    conn.execute_batch("COMMIT")?;
    conn.execute_batch("DETACH DATABASE src")?;
    eprintln!("done in {:.1}s -> {}", started.elapsed().as_secs_f64(), out.display());
    Ok(())
}

/// Catalog tables that a legacy source does not have; they are filled by
/// interning while copying the item tables.
fn legacy_catalog(table: &str) -> bool {
    matches!(table, "sys_commodities" | "sys_modules" | "sys_ships")
}

fn table_exists(conn: &Connection, schema: &str, table: &str) -> Result<bool> {
    Ok(conn
        .prepare(&format!(
            "SELECT 1 FROM {schema}.sqlite_master WHERE type='table' AND name=?1"
        ))?
        .exists([table])?)
}

/// Copy rows matching `filter`. The output table's column list drives the
/// copy; every column must exist in the source, or we fail loudly -- a
/// double-quoted name SQLite cannot resolve silently becomes a string
/// literal, which is how a first run produced 50 M rows of
/// `commodity_id = "commodity_id"`.
///
/// Sources in the pre-compact layout (`sys_market(station_id, symbol, ...)`,
/// no catalog tables) are converted: symbols are interned into the catalog
/// table and the item table is written with ids.
fn copy(conn: &Connection, table: &str, filter: &str) -> Result<()> {
    let out_cols = columns(conn, "galaxy", table)?;
    let src_cols = columns(conn, "src", table)?;
    if let Some((id_col, catalog)) = legacy_item_table(table) {
        if !src_cols.iter().any(|c| c == id_col) && src_cols.iter().any(|c| c == "symbol") {
            let out_cols: Vec<String> = out_cols
                .into_iter()
                .filter(|c| c == id_col || src_cols.contains(c))
                .collect();
            return copy_legacy_items(conn, table, id_col, catalog, &out_cols, &src_cols, filter);
        }
    }
    let list = quoted(&common_columns(conn, table)?);
    let n = conn
        .execute(
            &format!(
                "INSERT OR REPLACE INTO galaxy.{table} ({list}) SELECT {list} FROM src.{table} WHERE {filter}"
            ),
            [],
        )
        .with_context(|| format!("copying {table}"))?;
    eprintln!("{table:>22}: {n} rows");
    Ok(())
}

/// Item tables whose compact form points at a catalog by id, and the
/// catalog each one interns its symbols into.
fn legacy_item_table(table: &str) -> Option<(&'static str, &'static str)> {
    match table {
        "sys_market" => Some(("commodity_id", "sys_commodities")),
        "sys_outfitting" => Some(("module_id", "sys_modules")),
        "sys_shipyard" => Some(("ship_id", "sys_ships")),
        _ => None,
    }
}

fn copy_legacy_items(
    conn: &Connection,
    table: &str,
    id_col: &str,
    catalog: &str,
    out_cols: &[String],
    src_cols: &[String],
    filter: &str,
) -> Result<()> {
    // Intern every symbol the filtered rows use. Legacy rows carry name (and
    // category for commodities); keep whatever the catalog can hold.
    let catalog_cols = columns(conn, "galaxy", catalog)?;
    let carry: Vec<&String> = catalog_cols
        .iter()
        .filter(|c| *c != "id" && src_cols.contains(c))
        .collect();
    let carry_list = carry.iter().map(|c| format!("\"{c}\"")).collect::<Vec<_>>().join(", ");
    let carry_min = carry
        .iter()
        .filter(|c| **c != "symbol")
        .map(|c| format!("MIN(\"{c}\") AS \"{c}\""))
        .collect::<Vec<_>>();
    let select = std::iter::once("lower(symbol) AS symbol".to_string())
        .chain(carry_min)
        .collect::<Vec<_>>()
        .join(", ");
    let interned = conn
        .execute(
            &format!(
                "INSERT OR IGNORE INTO galaxy.{catalog} ({carry_list})
                 SELECT {select} FROM src.{table} WHERE {filter} GROUP BY lower(symbol)"
            ),
            [],
        )
        .with_context(|| format!("interning {table} symbols into {catalog}"))?;
    // Legacy `updated` is text ("2026-06-16 09:18:48+00" or RFC 3339); the
    // compact column is an integer epoch, so convert while copying.
    let rest: Vec<String> = out_cols
        .iter()
        .filter(|c| *c != id_col)
        .map(|c| {
            if c == "updated" {
                "CAST(strftime('%s', substr(s.\"updated\", 1, 19)) AS INTEGER) AS \"updated\"".to_string()
            } else {
                format!("s.\"{c}\"")
            }
        })
        .collect();
    let out_list = quoted(out_cols);
    let src_list = std::iter::once("c.id".to_string())
        .chain(rest)
        .collect::<Vec<_>>()
        .join(", ");
    // Column order: id first in the SELECT, so put it first in the target list.
    let out_list = {
        let mut cols: Vec<&String> = out_cols.iter().collect();
        cols.sort_by_key(|c| *c != id_col);
        let _ = out_list;
        quoted(&cols.into_iter().cloned().collect::<Vec<_>>())
    };
    let n = conn
        .execute(
            &format!(
                "INSERT OR REPLACE INTO galaxy.{table} ({out_list})
                 SELECT {src_list} FROM src.{table} s
                 JOIN galaxy.{catalog} c ON c.symbol = lower(s.symbol)
                 WHERE {}",
                filter.replace("station_id", "s.station_id")
            ),
            [],
        )
        .with_context(|| format!("copying legacy {table}"))?;
    eprintln!("{table:>22}: {n} rows ({interned} symbols interned into {catalog}; legacy layout)");
    Ok(())
}

/// The output table's columns that the source also has. Columns the
/// source predates (added by later migrations) are reported and left NULL;
/// a source with none of the key columns is an error.
fn common_columns(conn: &Connection, table: &str) -> Result<Vec<String>> {
    let out_cols = columns(conn, "galaxy", table)?;
    let src_cols = columns(conn, "src", table)?;
    let (common, missing): (Vec<String>, Vec<String>) =
        out_cols.into_iter().partition(|c| src_cols.contains(c));
    if !missing.is_empty() {
        eprintln!("{table:>22}: source lacks {missing:?} (left NULL)");
    }
    if common.is_empty() {
        bail!("source {table} shares no columns with the output; source has {src_cols:?}");
    }
    Ok(common)
}

fn columns(conn: &Connection, schema: &str, table: &str) -> Result<Vec<String>> {
    Ok(conn
        .prepare(&format!("SELECT name FROM pragma_table_info('{table}', '{schema}')"))?
        .query_map([], |r| r.get::<_, String>(0))?
        .collect::<rusqlite::Result<_>>()?)
}

fn quoted(cols: &[String]) -> String {
    cols.iter().map(|c| format!("\"{c}\"")).collect::<Vec<_>>().join(", ")
}
