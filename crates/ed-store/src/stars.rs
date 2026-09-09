//! Main-star classes learned for systems the routing index has as unknown.
//!
//! The star index ships with a class per system where Spansh knew one;
//! the rest are filled in over time from Spansh `/system` lookups, EDSM
//! sweeps and the commander's own scans. Those learnings are persisted
//! here (`galaxy.star_overrides`) and taught back to the index at startup.

use anyhow::Context as _;
use anyhow::Result;
use rusqlite::{params, Connection};

/// One learned star. `class` is the `ed_galaxy::StarClass` debug name; this
/// crate stores it opaquely so it need not depend on the routing index.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StarOverride {
    pub id64: i64,
    pub name: Option<String>,
    /// The source's own star-type string (`"K (Yellow-Orange) Star"`).
    pub subtype: String,
    pub class: String,
    pub scoopable: bool,
    /// Where it was learned: `journal`, `edsm`, `spansh /system`, ...
    pub source: String,
}

/// Record a learned star, replacing any earlier record for the system.
pub fn save(conn: &Connection, star: &StarOverride) -> Result<()> {
    conn.prepare_cached(
        "INSERT OR REPLACE INTO star_overrides
             (id64, name, subtype, class, scoopable, source, fetched_at)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, strftime('%Y-%m-%dT%H:%M:%SZ','now'))",
    )?
    .execute(params![
        star.id64,
        star.name,
        star.subtype,
        star.class,
        star.scoopable as i64,
        star.source
    ])?;
    Ok(())
}

/// Record a learned star only if nothing is known for the system yet --
/// for weaker evidence (a route planner's neutron flag) that must not
/// overwrite a real scan.
pub fn save_if_unknown(conn: &Connection, star: &StarOverride) -> Result<bool> {
    let inserted = conn
        .prepare_cached(
            "INSERT OR IGNORE INTO star_overrides
                 (id64, name, subtype, class, scoopable, source, fetched_at)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, strftime('%Y-%m-%dT%H:%M:%SZ','now'))",
        )?
        .execute(params![
            star.id64,
            star.name,
            star.subtype,
            star.class,
            star.scoopable as i64,
            star.source
        ])?;
    Ok(inserted > 0)
}

/// Every learned `(id64, class, subtype)`, for teaching the index at
/// startup. `class` is the stable enum name and is the key; `subtype` is
/// the raw source string, kept only as a fallback.
pub fn load_all(conn: &Connection) -> Result<Vec<(i64, String, Option<String>)>> {
    let mut stmt = conn.prepare("SELECT id64, class, subtype FROM star_overrides")?;
    let rows = stmt.query_map([], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)))?;
    Ok(rows.collect::<rusqlite::Result<Vec<_>>>()?)
}

/// How many stars a source has taught us, and how many of them are
/// neutron stars (the ones that change routing).
pub fn counts_for_source(conn: &Connection, source: &str) -> Result<(i64, i64)> {
    Ok(conn.query_row(
        "SELECT COUNT(*), COALESCE(SUM(class = 'Neutron'), 0) FROM star_overrides WHERE source = ?1",
        [source],
        |r| Ok((r.get(0)?, r.get(1)?)),
    )?)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn db() -> Connection {
        let conn = Connection::open_in_memory().unwrap();
        crate::schema::migrate(&conn).unwrap();
        crate::schema::attach_galaxy(&conn, None).unwrap();
        conn
    }

    fn star(id64: i64, class: &str, source: &str) -> StarOverride {
        StarOverride {
            id64,
            name: Some(format!("System {id64}")),
            subtype: format!("{class} Star"),
            class: class.to_string(),
            scoopable: class == "K",
            source: source.to_string(),
        }
    }

    #[test]
    fn star_overrides_round_trip_and_weak_evidence_never_overwrites() {
        let conn = db();
        save(&conn, &star(1, "K", "journal")).unwrap();
        save(&conn, &star(2, "Neutron", "edsm")).unwrap();
        assert!(!save_if_unknown(&conn, &star(1, "Neutron", "spansh route flag")).unwrap());
        assert!(save_if_unknown(&conn, &star(3, "Neutron", "spansh route flag")).unwrap());

        let mut all = load_all(&conn).unwrap();
        all.sort();
        assert_eq!(
            all,
            vec![
                (1, "K".to_string(), Some("K Star".to_string())),
                (2, "Neutron".to_string(), Some("Neutron Star".to_string())),
                (3, "Neutron".to_string(), Some("Neutron Star".to_string()))
            ]
        );
        assert_eq!(counts_for_source(&conn, "edsm").unwrap(), (1, 1));
        assert_eq!(counts_for_source(&conn, "journal").unwrap(), (1, 0));
    }
}

/// What a stars-product hydration did.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, serde::Serialize)]
pub struct StarsHydration {
    pub records: u64,
    pub written: u64,
    pub kept_journal: u64,
}

/// Merge a `stars` product (an EBEX with a stars section) into
/// `star_overrides`. A row the commander's own journal produced is
/// first-hand and is never replaced; anything else takes the server's
/// class. The caller teaches the index afterwards (`learn_class` per row,
/// or a reload of the overrides).
pub fn hydrate_ebex(conn: &Connection, bytes: &[u8]) -> Result<StarsHydration> {
    hydrate_ebex_cancellable(conn, bytes, &|| false)
}

/// [`hydrate_ebex`] that polls `cancelled` as it writes (every 2^16
/// records). A cancelled hydrate rolls back whole — the stars product is
/// one transaction — and the next sync redoes it from the artifact.
pub fn hydrate_ebex_cancellable(
    conn: &Connection,
    bytes: &[u8],
    cancelled: &(dyn Fn() -> bool + Sync),
) -> Result<StarsHydration> {
    ed_ebex::validate_snapshot(bytes)?;
    let section = ed_ebex::section(bytes, ed_ebex::SECTION_STARS)?.context("EBEX has no stars section")?;
    ed_ebex::validate_stars_section(section)?;
    let tx = conn.unchecked_transaction()?;
    let mut stats = StarsHydration::default();
    {
        let mut upsert = tx.prepare(
            "INSERT INTO star_overrides (id64, name, subtype, class, scoopable, source, fetched_at)
             VALUES (?1, NULL, ?2, ?3, ?4, 'server', strftime('%Y-%m-%dT%H:%M:%SZ', 'now'))
             ON CONFLICT(id64) DO UPDATE SET
                 subtype = excluded.subtype, class = excluded.class, scoopable = excluded.scoopable,
                 source = excluded.source, fetched_at = excluded.fetched_at
             WHERE star_overrides.source IS NOT 'journal'",
        )?;
        for record in ed_ebex::star_records(section)? {
            if stats.records & 0xffff == 0 && cancelled() {
                return Err(crate::market::HydrationCancelled.into());
            }
            stats.records += 1;
            let class = ed_galaxy_class_name(record.class);
            let changed = upsert.execute(rusqlite::params![record.address, class, class, record.scoopable as i64])?;
            if changed == 0 {
                stats.kept_journal += 1;
            } else {
                stats.written += 1;
            }
        }
    }
    tx.commit()?;
    Ok(stats)
}

/// The class name `star_overrides.class` stores, from a stars-section code.
/// Mirrors `ed_galaxy::StarClass::name` without the dependency.
fn ed_galaxy_class_name(code: u8) -> &'static str {
    match code {
        1 => "O", 2 => "B", 3 => "A", 4 => "F", 5 => "G", 6 => "K", 7 => "M", 8 => "L", 9 => "T", 10 => "Y",
        11 => "Proto", 12 => "Exotic", 13 => "WhiteDwarf", 14 => "Neutron", 15 => "BlackHole",
        _ => "Unknown",
    }
}

#[cfg(test)]
mod hydrate_tests {
    use super::*;

    fn stars_ebex(records: &[(i64, u8, bool)]) -> Vec<u8> {
        let mut bytes = Vec::new();
        for (address, class, scoopable) in records {
            ed_ebex::StarRecord { address: *address, class: *class, scoopable: *scoopable, observed_at: 1_700_000_000 }.encode_into(&mut bytes);
        }
        ed_ebex::encode_snapshot(
            ed_ebex::SnapshotHeader { sequence: 1, created_at: 1, watermark: 1 },
            vec![ed_ebex::Section { id: ed_ebex::SECTION_STARS, schema: ed_ebex::STAR_SCHEMA_V1, required: false, record_count: records.len() as u64, record_size: ed_ebex::STAR_RECORD_BYTES, records: bytes, auxiliary: vec![] }],
        )
        .unwrap()
    }

    /// A cancelled stars hydrate rolls back whole and the rerun lands
    /// everything (maintainer ruling 2026-09-04, same contract as market).
    #[test]
    fn a_cancelled_stars_hydrate_rolls_back_and_reruns() {
        let conn = Connection::open_in_memory().unwrap();
        crate::schema::migrate(&conn).unwrap();
        crate::schema::attach_galaxy(&conn, None).unwrap();
        let bytes = stars_ebex(&[(1, 14, false), (2, 6, true)]);
        let error = hydrate_ebex_cancellable(&conn, &bytes, &|| true).unwrap_err();
        assert!(error.downcast_ref::<crate::market::HydrationCancelled>().is_some(), "{error}");
        let rows: i64 = conn.query_row("SELECT COUNT(*) FROM star_overrides", [], |r| r.get(0)).unwrap();
        assert_eq!(rows, 0, "a cancelled single-transaction hydrate leaves nothing");
        let stats = hydrate_ebex_cancellable(&conn, &bytes, &|| false).unwrap();
        assert_eq!(stats.records, 2);
    }

    /// The server's classes land in the overrides, but a class the
    /// commander scanned first-hand is never replaced.
    #[test]
    fn server_stars_merge_without_overriding_the_journal() {
        let conn = Connection::open_in_memory().unwrap();
        crate::schema::migrate(&conn).unwrap();
        crate::schema::attach_galaxy(&conn, None).unwrap();
        save(&conn, &StarOverride { id64: 5, name: Some("Mine".into()), subtype: "K (Yellow-Orange) Star".into(), class: "K".into(), scoopable: true, source: "journal".into() }).unwrap();
        save(&conn, &StarOverride { id64: 6, name: None, subtype: "M".into(), class: "M".into(), scoopable: true, source: "edsm".into() }).unwrap();
        let stats = hydrate_ebex(&conn, &stars_ebex(&[(5, 14, false), (6, 14, false), (7, 13, false)])).unwrap();
        assert_eq!(stats, StarsHydration { records: 3, written: 2, kept_journal: 1 });
        let rows = load_all(&conn).unwrap();
        let class_of = |id: i64| rows.iter().find(|r| r.0 == id).map(|r| r.1.clone()).unwrap();
        assert_eq!(class_of(5), "K", "the journal's scan stands");
        assert_eq!(class_of(6), "Neutron", "an older non-journal class is replaced");
        assert_eq!(class_of(7), "WhiteDwarf");
        let source: String = conn.query_row("SELECT source FROM star_overrides WHERE id64 = 7", [], |r| r.get(0)).unwrap();
        assert_eq!(source, "server");
    }
}
