//! Mining page queries (maintainer shape, ledgered 2026-09-06): the
//! commander's own MARKS first, then ring HOTSPOTS (material-named,
//! exact — from the galaxy import), then could-have BODIES (landable,
//! ranked by surface-material concentration). Marks are local-only,
//! permanently; the shared leg was buried on the poison-data premise.
//!
//! Reserve level ("pristine…") is NOT in the current galaxy import;
//! the filter arrives with the planned re-import (ledger candidate)
//! and, until then, from the commander's own Scan events. Nothing here
//! guesses it.

use anyhow::Result;
use rusqlite::Connection;
use serde::Serialize;

#[derive(Debug, Clone, Serialize)]
pub struct Mark {
    pub id: i64,
    pub label: String,
    pub system: String,
    pub body: Option<String>,
    pub note: Option<String>,
    pub created_ts: String,
    pub distance_ly: Option<f64>,
    pub station: Option<String>,
    pub latitude: Option<f64>,
    pub longitude: Option<f64>,
}

impl Mark {
    /// The finest place this mark actually names, for one line of UI.
    /// Never invents precision it does not hold.
    pub fn where_text(&self) -> String {
        let mut s = self.system.clone();
        if let Some(station) = self.station.as_deref() {
            s.push_str(&format!(" — {station}"));
        }
        if let Some(body) = self.body.as_deref() {
            s.push_str(&format!(" — {body}"));
        }
        if let (Some(lat), Some(lon)) = (self.latitude, self.longitude) {
            s.push_str(&format!(" @ {lat:.4}, {lon:.4}"));
        }
        s
    }
}

/// Where the commander is, at the finest grain the journal currently
/// offers — the single source a bookmark records from (the resolver
/// pattern, maintainer-ratified 2026-09-06).
///
/// Every field below `system` is present only when the game said so:
/// `station` while docked, `body` whenever `Status.json` names one, and
/// `latitude`/`longitude` only while the surface fix is live. A `None`
/// means "never told", never a guess — a bookmark that sends the
/// commander back to the wrong crater is worse than one that admits it
/// only knows the system.
#[derive(Debug, Clone, Default, Serialize)]
pub struct Fix {
    pub system: String,
    pub system_id64: Option<i64>,
    pub station: Option<String>,
    pub body: Option<String>,
    pub latitude: Option<f64>,
    pub longitude: Option<f64>,
}

impl Fix {
    /// How precise this fix is, for the UI to say plainly what will be
    /// saved before the commander commits to it.
    pub fn grain(&self) -> &'static str {
        match (self.latitude.is_some() && self.longitude.is_some(), self.body.is_some(), self.station.is_some()) {
            (true, _, _) => "surface",
            (_, true, _) => "body",
            (_, _, true) => "station",
            _ => "system",
        }
    }
}

/// Read the current fix: the derived `location` row for system and
/// station, `Status.json` for the body and the surface position.
pub fn here(conn: &Connection) -> Result<Fix> {
    let (system, system_id64, docked, station): (Option<String>, Option<i64>, bool, Option<String>) = conn
        .query_row(
            "SELECT system_name, system_address, COALESCE(docked, 0), station_name FROM location WHERE id = 1",
            [],
            |r| Ok((r.get(0)?, r.get(1)?, r.get::<_, i64>(2)? != 0, r.get(3)?)),
        )
        .unwrap_or((None, None, false, None));
    let mut fix = Fix {
        system: system.unwrap_or_default(),
        system_id64,
        station: if docked { station } else { None },
        ..Fix::default()
    };
    if let Ok(Some(raw)) = crate::session::snapshot_raw(conn, "Status.json") {
        if let Ok(v) = serde_json::from_str::<serde_json::Value>(&raw) {
            let num = |k: &str| v.get(k).and_then(serde_json::Value::as_f64);
            fix.body = v
                .get("BodyName")
                .and_then(serde_json::Value::as_str)
                .map(str::to_string)
                .filter(|b| !b.is_empty());
            // Latitude and longitude are only meaningful together, and
            // only while the game is publishing a surface fix at all.
            if let (Some(lat), Some(lon)) = (num("Latitude"), num("Longitude")) {
                fix.latitude = Some(lat);
                fix.longitude = Some(lon);
            }
        }
    }
    Ok(fix)
}

/// Save a bookmark at the given fix. Everything but the label and the
/// system is optional, because the game does not always say.
pub fn mark_add(conn: &Connection, label: &str, fix: &Fix, body: Option<&str>, note: Option<&str>) -> Result<i64> {
    // A body typed by the commander wins over the one the game reported:
    // they may be naming a ring or a site the fix cannot see.
    let body = body.map(str::trim).filter(|b| !b.is_empty()).map(str::to_string).or_else(|| fix.body.clone());
    conn.execute(
        "INSERT INTO user_marks (label, system, system_id64, body, note, created_ts, station, latitude, longitude)
         VALUES (?1, ?2, ?3, ?4, ?5, strftime('%Y-%m-%dT%H:%M:%SZ', 'now'), ?6, ?7, ?8)",
        rusqlite::params![
            label.trim(),
            fix.system.trim(),
            fix.system_id64,
            body,
            note,
            fix.station,
            fix.latitude,
            fix.longitude
        ],
    )?;
    Ok(conn.last_insert_rowid())
}

pub fn mark_remove(conn: &Connection, id: i64) -> Result<bool> {
    Ok(conn.execute("DELETE FROM user_marks WHERE id = ?1", [id])? > 0)
}

/// Marks matching the search text (empty text = all), nearest first
/// when the origin and the marked system's coordinates are both known;
/// unknown distances sink to the end rather than pretending zero.
pub fn marks_near(
    conn: &Connection,
    origin: Option<(f64, f64, f64)>,
    text: &str,
) -> Result<Vec<Mark>> {
    let mut stmt = conn.prepare(
        "SELECT m.id, m.label, m.system, m.body, m.note, m.created_ts,
                CASE WHEN ?2 IS NULL OR sy.x IS NULL THEN NULL
                     ELSE ((sy.x-?2)*(sy.x-?2) + (sy.y-?3)*(sy.y-?3) + (sy.z-?4)*(sy.z-?4))
                END AS distance_ly,
                m.station, m.latitude, m.longitude
         FROM user_marks m
         LEFT JOIN sys_systems sy
                ON sy.id64 = m.system_id64 OR (m.system_id64 IS NULL AND sy.name = m.system COLLATE NOCASE)
         WHERE ?1 = '' OR m.label LIKE '%' || ?1 || '%' OR m.note LIKE '%' || ?1 || '%'
         ORDER BY distance_ly IS NULL, distance_ly, m.created_ts DESC",
    )?;
    let (ox, oy, oz) = match origin {
        Some((x, y, z)) => (Some(x), Some(y), Some(z)),
        None => (None, None, None),
    };
    let rows = stmt.query_map(rusqlite::params![text.trim(), ox, oy, oz], |r| {
        Ok(Mark {
            id: r.get(0)?,
            label: r.get(1)?,
            system: r.get(2)?,
            body: r.get(3)?,
            note: r.get(4)?,
            created_ts: r.get(5)?,
            distance_ly: r.get::<_, Option<f64>>(6)?.map(f64::sqrt),
            station: r.get(7)?,
            latitude: r.get(8)?,
            longitude: r.get(9)?,
        })
    })?;
    Ok(rows.collect::<rusqlite::Result<Vec<_>>>()?)
}

/// Spelling-blind comparison key: case, spaces, punctuation and a
/// trailing plural all vanish, so "Low Temperature Diamonds" finds the
/// stored `LowTemperatureDiamond`.
pub fn material_key(name: &str) -> String {
    let mut k: String = name
        .chars()
        .filter(|c| c.is_ascii_alphanumeric())
        .map(|c| c.to_ascii_lowercase())
        .collect();
    if k.ends_with('s') {
        k.pop();
    }
    k
}

/// Laser-mined goods with no hotspot mechanic, and the ring type that
/// carries them. Deliberately small and well-known; anything not here
/// and not in the vocabulary gets the honest "can't map this" copy.
pub fn ring_type_for(name: &str) -> Option<(&'static str, &'static str)> {
    match material_key(name).as_str() {
        "gold" | "silver" | "palladium" | "osmium" | "bertrandite" | "indite" | "gallite"
        | "praseodymium" | "samarium" => Some(("Metallic", "laser-mined from Metallic rings")),
        "bauxite" | "cobalt" | "rutile" => Some(("Rocky", "laser-mined from Rocky rings")),
        "hydrogenperoxide" | "liquidoxygen" | "methanolmonohydratecrystal"
        | "lithiumhydroxide" | "water" => Some(("Icy", "mined from Icy rings")),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn store() -> Connection {
        let conn = Connection::open_in_memory().unwrap();
        crate::schema::migrate(&conn).unwrap();
        crate::schema::attach_galaxy(&conn, None).unwrap();
        conn.execute_batch(
            "INSERT INTO sys_systems (id64, name, x, y, z) VALUES
                 (1, 'Sol', 0, 0, 0), (2, 'Near', 10, 0, 0), (3, 'Far', 400, 0, 0);
             INSERT INTO sys_bodies (id64, system_id64, name, sub_type, is_landable, distance_to_arrival, gravity) VALUES
                 (101, 2, 'Near 2', 'High metal content world', 1, 300, 0.4),
                 (102, 2, 'Near 3 A', 'Icy body', 0, 900, 1.1),
                 (103, 3, 'Far 1', 'High metal content world', 1, 50, 0.3);
             INSERT INTO sys_body_materials (body_id64, material, percent) VALUES
                 (101, 'Iridium', 1.9), (102, 'Iridium', 3.5), (103, 'Iridium', 2.4),
                 (101, 'Iron', 20.0);
             INSERT INTO sys_rings (body_id64, name, type) VALUES
                 (102, 'Near 3 A Ring', 'Icy');
             INSERT INTO sys_ring_hotspots (body_id64, ring_name, material, count) VALUES
                 (102, 'Near 3 A Ring', 'Platinum', 2),
                 (102, 'Near 3 A Ring', 'Low Temperature Diamonds', 1),
                 (103, 'Far 1 Ring', 'Platinum', 5);",
        )
        .unwrap();
        conn
    }

    /// Marks: added, listed nearest-first (unknown distances sink, not
    /// zero), filtered by text, removed.
    #[test]
    fn marks_are_personal_breadcrumbs_with_honest_distances() {
        let conn = store();
        let at = |system: &str, id64: Option<i64>| Fix {
            system: system.into(),
            system_id64: id64,
            ..Fix::default()
        };
        mark_add(&conn, "Iridium", &at("Far", Some(3)), Some("Far 1"), Some("22 sites, gold too")).unwrap();
        mark_add(&conn, "Gold", &at("Near", None), None, None).unwrap();
        mark_add(&conn, "Iridium", &at("Uncharted Depths", None), None, None).unwrap();
        let all = marks_near(&conn, Some((0.0, 0.0, 0.0)), "").unwrap();
        assert_eq!(all.len(), 3);
        assert_eq!(all[0].system, "Near");
        assert_eq!(all[1].system, "Far");
        assert!(all[2].distance_ly.is_none(), "unknown system sinks with no distance");
        let iridium = marks_near(&conn, Some((0.0, 0.0, 0.0)), "irid").unwrap();
        assert_eq!(iridium.len(), 2);
        assert!(mark_remove(&conn, iridium[0].id).unwrap());
        assert_eq!(marks_near(&conn, None, "irid").unwrap().len(), 1);
    }

    /// Maintainer, 2026-09-06, widening marks into bookmarks: "really it should
    /// be any bookmark for a thing. The more granular position we can get
    /// the better." A bookmark records the FINEST grain the game offered
    /// and NOT ONE STEP MORE — a null is "never told", never a guess,
    /// because sending the commander back to the wrong crater is worse
    /// than admitting we only know the system.
    #[test]
    fn a_bookmark_keeps_every_grain_the_game_gave_and_invents_none() {
        let conn = store();
        let surface = Fix {
            system: "Far".into(),
            system_id64: Some(3),
            station: None,
            body: Some("Far 1 A".into()),
            latitude: Some(-22.4137),
            longitude: Some(118.7642),
        };
        mark_add(&conn, "Antimony", &surface, None, Some("brain trees")).unwrap();
        // System only: in supercruise the game names nothing else.
        mark_add(&conn, "Passing thought", &Fix { system: "Near".into(), ..Fix::default() }, None, None).unwrap();

        let all = marks_near(&conn, Some((0.0, 0.0, 0.0)), "").unwrap();
        let anti = all.iter().find(|m| m.label == "Antimony").expect("the surface bookmark");
        assert_eq!(anti.body.as_deref(), Some("Far 1 A"));
        assert_eq!(anti.latitude, Some(-22.4137));
        assert_eq!(anti.longitude, Some(118.7642));
        assert!(anti.where_text().contains("Far 1 A @ -22.4137, 118.7642"), "{}", anti.where_text());

        let thought = all.iter().find(|m| m.label == "Passing thought").unwrap();
        assert!(thought.body.is_none() && thought.latitude.is_none(), "nothing is invented");
        assert_eq!(thought.where_text(), "Near", "the system alone, said plainly");

        // A typed body beats the fix's: the commander may be naming a
        // ring or a site the surface fix cannot see.
        mark_add(&conn, "Platinum", &surface, Some("Far 1 A Ring"), None).unwrap();
        let ring = marks_near(&conn, None, "Platinum").unwrap();
        assert_eq!(ring[0].body.as_deref(), Some("Far 1 A Ring"));
        assert_eq!(ring[0].latitude, Some(-22.4137), "the position it was taken at survives");

        // The grain ladder, most precise first.
        assert_eq!(surface.grain(), "surface");
        assert_eq!(Fix { system: "S".into(), body: Some("B".into()), ..Fix::default() }.grain(), "body");
        assert_eq!(Fix { system: "S".into(), station: Some("P".into()), ..Fix::default() }.grain(), "station");
        assert_eq!(Fix { system: "S".into(), ..Fix::default() }.grain(), "system");

        // Bookmarks are searched by label OR note, so a material farm
        // found by name is found again by what it yields.
        assert_eq!(marks_near(&conn, None, "brain").unwrap().len(), 1, "the note is searchable too");
    }
}
