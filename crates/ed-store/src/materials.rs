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
    let mut stmt = conn.prepare(
        "SELECT event, raw FROM events
         WHERE event IN ('FSDJump','Location','CarrierJump','ApproachBody','LeaveBody','Touchdown','Liftoff','SupercruiseEntry','MaterialCollected')
         ORDER BY file, offset",
    )?;
    let rows = stmt.query_map([], |r| Ok((r.get::<_, String>(0)?, r.get::<_, String>(1)?)))?;

    let mut system: Option<String> = None;
    let mut body: Option<String> = None;
    let mut acc: HashMap<(String, Option<String>), WitnessedSource> = HashMap::new();
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
            "Liftoff" => {}
            "MaterialCollected" => {
                let name = s("Name_Localised")
                    .or_else(|| s("Name"))
                    .unwrap_or_default();
                let sym = s("Name").unwrap_or_default();
                if name.to_lowercase() != want && sym.to_lowercase() != want {
                    continue;
                }
                let Some(sys) = system.clone() else { continue };
                let count = v.get("Count").and_then(|c| c.as_i64()).unwrap_or(1);
                let ts = s("timestamp").unwrap_or_default();
                let e = acc
                    .entry((sys.clone(), body.clone()))
                    .or_insert(WitnessedSource {
                        system: sys,
                        body: body.clone(),
                        count: 0,
                        pickups: 0,
                        last: ts.clone(),
                    });
                e.count += count;
                e.pickups += 1;
                if ts > e.last {
                    e.last = ts;
                }
            }
            _ => {}
        }
    }
    let mut out: Vec<_> = acc.into_values().collect();
    out.sort_by(|a, b| b.count.cmp(&a.count).then(b.last.cmp(&a.last)));
    Ok(out)
}
