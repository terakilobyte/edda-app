//! Display metadata from the community-maintained FDevIDs tables
//! (github.com/EDCD/FDevIDs): Frontier's own IDs, symbols, display names
//! and categories, kept current by the Community Developers.
//!
//! The data itself is baked into the binary (`fdev_data`, regenerated
//! from the CSVs as Frontier adds items — the user's call: encode the ID
//! tables in source and update over time); `hydrate --fdev-ids <csv>`
//! remains for picking up a fresh download without a rebuild.
//! Attribution: EDCD/FDevIDs.

use std::path::Path;

use anyhow::{ensure, Context, Result};
use sqlx::PgPool;

/// One commodity row: canonical symbol, display name, category.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CommodityMeta {
    pub symbol: String,
    pub name: String,
    pub category: String,
}

/// Parse FDevIDs `commodity.csv` (header `id,symbol,category,name` —
/// category comes BEFORE name in the source). Rows with a missing symbol
/// are rejected; quoted fields (a name containing a comma) are handled.
/// Symbols are lowercased to the ingest path's canonical form
/// (`postgres.rs` stores `commodity.name.to_lowercase()`): FDevIDs writes
/// `AdvancedCatalysers` where every market row says `advancedcatalysers`,
/// and a case-sensitive upsert would shelve the metadata on a duplicate
/// row no market row references.
pub fn parse_commodity_csv(text: &str) -> Result<Vec<CommodityMeta>> {
    let mut lines = text.lines();
    let header = lines.next().context("empty FDevIDs commodity file")?;
    let columns = split_csv_line(header);
    let index_of = |want: &str| -> Result<usize> {
        columns
            .iter()
            .position(|c| c.eq_ignore_ascii_case(want))
            .with_context(|| format!("FDevIDs commodity header has no {want:?} column: {header}"))
    };
    let (symbol_at, category_at, name_at) = (
        index_of("symbol")?,
        index_of("category")?,
        index_of("name")?,
    );
    let mut out = Vec::new();
    for line in lines {
        if line.trim().is_empty() {
            continue;
        }
        let fields = split_csv_line(line);
        let field = |at: usize| fields.get(at).map(String::as_str).unwrap_or("").trim();
        let symbol = field(symbol_at);
        ensure!(
            !symbol.is_empty(),
            "FDevIDs commodity row without a symbol: {line}"
        );
        out.push(CommodityMeta {
            symbol: symbol.to_lowercase(),
            name: field(name_at).to_string(),
            category: field(category_at).to_string(),
        });
    }
    Ok(out)
}

/// Split one CSV line, honouring double-quoted fields with `""` escapes.
fn split_csv_line(line: &str) -> Vec<String> {
    let mut fields = Vec::new();
    let mut current = String::new();
    let mut quoted = false;
    let mut chars = line.chars().peekable();
    while let Some(c) = chars.next() {
        match c {
            '"' if quoted && chars.peek() == Some(&'"') => {
                chars.next();
                current.push('"');
            }
            '"' => quoted = !quoted,
            ',' if !quoted => fields.push(std::mem::take(&mut current)),
            other => current.push(other),
        }
    }
    fields.push(current);
    fields
}

/// Upsert display names and categories onto the commodities table from
/// the baked-in [`crate::fdev_data::COMMODITY_METADATA`] table — the
/// default, no file needed. Returns rows written.
pub async fn hydrate_builtin(pool: &PgPool) -> Result<u64> {
    let rows: Vec<CommodityMeta> = crate::fdev_data::COMMODITY_METADATA
        .iter()
        .map(|(symbol, name, category)| CommodityMeta {
            symbol: (*symbol).into(),
            name: (*name).into(),
            category: (*category).into(),
        })
        .collect();
    upsert(pool, &rows).await
}

/// Upsert from a downloaded FDevIDs `commodity.csv` — for picking up new
/// commodities without a rebuild. Returns rows written.
pub async fn hydrate_commodities(pool: &PgPool, path: &Path) -> Result<u64> {
    let text =
        std::fs::read_to_string(path).with_context(|| format!("reading {}", path.display()))?;
    let rows = parse_commodity_csv(&text)?;
    ensure!(!rows.is_empty(), "no commodity rows in {}", path.display());
    upsert(pool, &rows).await
}

async fn upsert(pool: &PgPool, rows: &[CommodityMeta]) -> Result<u64> {
    let symbols: Vec<&str> = rows.iter().map(|r| r.symbol.as_str()).collect();
    let names: Vec<&str> = rows.iter().map(|r| r.name.as_str()).collect();
    let categories: Vec<&str> = rows.iter().map(|r| r.category.as_str()).collect();
    let written = sqlx::query(
        "INSERT INTO commodities (symbol, name, category) \
         SELECT symbol, name, category \
         FROM unnest($1::text[], $2::text[], $3::text[]) AS t(symbol, name, category) \
         ON CONFLICT (symbol) DO UPDATE SET \
           name = EXCLUDED.name, category = EXCLUDED.category",
    )
    .bind(&symbols)
    .bind(&names)
    .bind(&categories)
    .execute(pool)
    .await?
    .rows_affected();
    Ok(written)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The real header order — category before name — and a quoted name
    /// with a comma must both survive. Symbols come out lowercased: the
    /// ingest path stores every symbol lowercase (postgres.rs), and a
    /// CamelCase FDevIDs symbol must land on that row, not beside it.
    #[test]
    fn parses_the_fdev_ids_header_order_and_quoting() {
        let rows = parse_commodity_csv(
            "id,symbol,category,name\n\
             128049154,Gold,Metals,Gold\n\
             128049177,Kinago,Weapons,\"Kinago Violins, Deluxe\"\n\
             \n\
             128064028,AncientRelic,Salvage,Ancient Relic\n",
        )
        .unwrap();
        assert_eq!(
            rows,
            vec![
                CommodityMeta {
                    symbol: "gold".into(),
                    name: "Gold".into(),
                    category: "Metals".into()
                },
                CommodityMeta {
                    symbol: "kinago".into(),
                    name: "Kinago Violins, Deluxe".into(),
                    category: "Weapons".into()
                },
                CommodityMeta {
                    symbol: "ancientrelic".into(),
                    name: "Ancient Relic".into(),
                    category: "Salvage".into()
                },
            ]
        );
    }

    /// The baked table is usable as-is: sorted, unique, every symbol
    /// non-empty, and it covers the classics.
    #[test]
    fn baked_table_is_well_formed() {
        let table = crate::fdev_data::COMMODITY_METADATA;
        assert!(
            table.len() >= 350,
            "expected the full catalog, got {}",
            table.len()
        );
        for pair in table.windows(2) {
            assert!(
                pair[0].0.to_ascii_lowercase() < pair[1].0.to_ascii_lowercase(),
                "sorted and unique: {} vs {}",
                pair[0].0,
                pair[1].0
            );
        }
        assert!(table.iter().all(|(s, _, _)| !s.is_empty()));
        // Symbols are baked in the ingest's canonical form — lowercase —
        // or hydrate_builtin recreates the orphan-row bug 83b65e5 fixed.
        assert!(
            table.iter().all(|(s, _, _)| *s == s.to_ascii_lowercase()),
            "baked symbols must be lowercase"
        );
        let gold = table.iter().find(|(s, _, _)| *s == "gold").unwrap();
        assert_eq!((gold.1, gold.2), ("Gold", "Metals"));
        let ltd = table
            .iter()
            .find(|(s, _, _)| *s == "lowtemperaturediamond")
            .unwrap();
        assert_eq!(ltd.1, "Low Temperature Diamonds");
    }

    #[test]
    fn rejects_files_that_are_not_the_commodity_table() {
        assert!(parse_commodity_csv("").is_err());
        assert!(
            parse_commodity_csv("id,name\n1,Gold\n").is_err(),
            "no symbol column"
        );
        assert!(
            parse_commodity_csv("id,symbol,category,name\n1,,Metals,Gold\n").is_err(),
            "empty symbol"
        );
    }
}
