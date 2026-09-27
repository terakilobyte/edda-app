//! Where the commander has actually picked up a material, from their own
//! journal: each `MaterialCollected` is attributed to the system (and body,
//! when landed or in orbit) they were at. First-hand, so it beats any guide
//! for "where did I get this last time".

use anyhow::Result;
use rusqlite::Connection;
use serde::Serialize;
use std::collections::HashMap;

#[derive(Debug, Clone, Serialize)]
pub struct WitnessedSource {
    pub system: String,
    pub body: Option<String>,
    pub count: i64,
    pub pickups: i64,
    pub last: String,
}

/// Places `material` (display name or symbol, case-insensitive) was collected,
/// most units first.
pub fn witnessed_sources(conn: &Connection, material: &str) -> Result<Vec<WitnessedSource>> {
    let want = material.to_lowercase();
    Ok(witnessed_sources_all(conn)?.into_iter().filter(|(k, _)| k.name == want || k.symbol == want).flat_map(|(_, v)| v).collect())
}

/// A material as the pickup names it: display name and symbol, lowercase.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct WitnessedKey {
    pub name: String,
    pub symbol: String,
}

/// Every material the commander has ever picked up and where, in one pass
/// over the journal (the build plan asks for several materials at once,
/// and each pass reads the whole event log). One row per system: pickups
/// made before the body was known (no ApproachBody yet) fold into the
/// same row, and the body most picked up on is the one named — two rows
/// for one site read as two sites (maintainer, 2026-09-27). Most units
/// first per material.
pub fn witnessed_sources_all(conn: &Connection) -> Result<Vec<(WitnessedKey, Vec<WitnessedSource>)>> {
    let mut stmt = conn.prepare(
        "SELECT event, raw FROM events
         WHERE event IN ('FSDJump','Location','CarrierJump','ApproachBody','LeaveBody','Touchdown','Liftoff','SupercruiseEntry','MaterialCollected')
         ORDER BY ts, file, offset",
    )?;
    let rows = stmt.query_map([], |r| Ok((r.get::<_, String>(0)?, r.get::<_, String>(1)?)))?;
    let mut system: Option<String> = None;
    let mut body: Option<String> = None;
    struct Acc {
        src: WitnessedSource,
        bodies: HashMap<String, i64>,
    }
    let mut acc: HashMap<WitnessedKey, HashMap<String, Acc>> = HashMap::new();
    for row in rows {
        let (event, raw) = row?;
        let v: serde_json::Value = match serde_json::from_str(&raw) {
            Ok(v) => v,
            Err(_) => continue,
        };
        let s = |k: &str| v.get(k).and_then(|x| x.as_str()).map(str::to_string);
        match event.as_str() {
            "FSDJump" | "Location" | "CarrierJump" => {
                system = s("StarSystem");
                body = None;
            }
            "ApproachBody" | "Touchdown" => body = s("Body").or(body.take()),
            "LeaveBody" | "SupercruiseEntry" => body = None,
            "MaterialCollected" => {
                let sym = s("Name").unwrap_or_default().to_lowercase();
                let name = s("Name_Localised").map(|n| n.to_lowercase()).unwrap_or_else(|| sym.clone());
                let Some(sys) = system.clone() else { continue };
                let count = v.get("Count").and_then(|c| c.as_i64()).unwrap_or(1);
                let ts = s("timestamp").unwrap_or_default();
                let e = acc
                    .entry(WitnessedKey { name, symbol: sym })
                    .or_default()
                    .entry(sys.clone())
                    .or_insert_with(|| Acc { src: WitnessedSource { system: sys, body: None, count: 0, pickups: 0, last: ts.clone() }, bodies: HashMap::new() });
                e.src.count += count;
                e.src.pickups += 1;
                if ts > e.src.last {
                    e.src.last = ts;
                }
                if let Some(b) = &body {
                    *e.bodies.entry(b.clone()).or_insert(0) += 1;
                }
            }
            _ => {}
        }
    }
    let mut out: Vec<(WitnessedKey, Vec<WitnessedSource>)> = acc
        .into_iter()
        .map(|(k, m)| {
            let mut v: Vec<_> = m
                .into_values()
                .map(|a| {
                    let body = a.bodies.iter().max_by(|x, y| x.1.cmp(y.1).then(y.0.cmp(x.0))).map(|(b, _)| b.clone());
                    WitnessedSource { body, ..a.src }
                })
                .collect();
            v.sort_by(|a, b| b.count.cmp(&a.count).then(b.last.cmp(&a.last)));
            (k, v)
        })
        .collect();
    out.sort_by(|a, b| a.0.name.cmp(&b.0.name));
    Ok(out)
}


#[cfg(test)]
mod tests {
    use super::*;
    use rusqlite::params;

    fn db() -> Connection {
        let conn = Connection::open_in_memory().unwrap();
        crate::schema::migrate(&conn).unwrap();
        crate::schema::attach_galaxy(&conn, None).unwrap();
        conn
    }

    fn event(conn: &Connection, offset: i64, ts: &str, event: &str, raw: &str) {
        conn.execute("INSERT INTO events (file, offset, ts, event, raw) VALUES ('Journal.1.log', ?1, ?2, ?3, ?4)", params![offset, ts, event, raw]).unwrap();
    }

    /// One site, one row: pickups before the body was known fold into the
    /// system's row and the body most picked up on is the one named
    /// (the report showed "Synuefe NL-N c23-4" twice, 2026-09-27).
    #[test]
    fn one_row_per_system_named_by_the_body_most_picked_up_on() {
        let conn = db();
        let pick = |n: i64| format!(r#"{{"timestamp":"2026-09-01T10:0{n}:00Z","event":"MaterialCollected","Category":"Manufactured","Name":"guardian_sentinel_wreckagecomponents","Name_Localised":"Guardian Wreckage Components","Count":{n}}}"#);
        event(&conn, 1, "2026-09-01T09:00:00Z", "FSDJump", r#"{"timestamp":"2026-09-01T09:00:00Z","event":"FSDJump","StarSystem":"Synuefe NL-N c23-4","SystemAddress":1}"#);
        event(&conn, 2, "2026-09-01T10:01:00Z", "MaterialCollected", &pick(1));
        event(&conn, 3, "2026-09-01T10:01:30Z", "ApproachBody", r#"{"timestamp":"2026-09-01T10:01:30Z","event":"ApproachBody","StarSystem":"Synuefe NL-N c23-4","Body":"Synuefe NL-N c23-4 B 3"}"#);
        event(&conn, 4, "2026-09-01T10:02:00Z", "MaterialCollected", &pick(2));
        event(&conn, 5, "2026-09-01T10:03:00Z", "MaterialCollected", &pick(3));
        let rows = witnessed_sources(&conn, "Guardian Wreckage Components").unwrap();
        assert_eq!(rows.len(), 1, "{rows:?}");
        assert_eq!((rows[0].system.as_str(), rows[0].body.as_deref(), rows[0].count, rows[0].pickups), ("Synuefe NL-N c23-4", Some("Synuefe NL-N c23-4 B 3"), 6, 3));
        assert_eq!(rows[0].last, "2026-09-01T10:03:00Z");
        // Found by symbol too.
        assert_eq!(witnessed_sources(&conn, "Guardian_Sentinel_WreckageComponents").unwrap().len(), 1);
    }
}
