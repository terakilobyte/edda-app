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
/// and each pass reads the whole event log). Most units first per material.
pub fn witnessed_sources_all(conn: &Connection) -> Result<Vec<(WitnessedKey, Vec<WitnessedSource>)>> {
    let mut stmt = conn.prepare(
        "SELECT event, raw FROM events
         WHERE event IN ('FSDJump','Location','CarrierJump','ApproachBody','LeaveBody','Touchdown','Liftoff','SupercruiseEntry','MaterialCollected')
         ORDER BY ts, file, offset",
    )?;
    let rows = stmt.query_map([], |r| Ok((r.get::<_, String>(0)?, r.get::<_, String>(1)?)))?;
    let mut system: Option<String> = None;
    let mut body: Option<String> = None;
    let mut acc: HashMap<WitnessedKey, HashMap<(String, Option<String>), WitnessedSource>> = HashMap::new();
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
                    .entry((sys.clone(), body.clone()))
                    .or_insert(WitnessedSource { system: sys, body: body.clone(), count: 0, pickups: 0, last: ts.clone() });
                e.count += count;
                e.pickups += 1;
                if ts > e.last {
                    e.last = ts;
                }
            }
            _ => {}
        }
    }
    let mut out: Vec<(WitnessedKey, Vec<WitnessedSource>)> = acc
        .into_iter()
        .map(|(k, m)| {
            let mut v: Vec<_> = m.into_values().collect();
            v.sort_by(|a, b| b.count.cmp(&a.count).then(b.last.cmp(&a.last)));
            (k, v)
        })
        .collect();
    out.sort_by(|a, b| a.0.name.cmp(&b.0.name));
    Ok(out)
}

