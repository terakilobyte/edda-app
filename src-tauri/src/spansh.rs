//! Import a route the commander plotted on spansh.co.uk.
//!
//! Spansh's plotters are fast because they precompute; ours is exact but
//! slow over 20,000 ly. Until that gap closes, a pasted results link brings
//! their hop list into the Route tab and HUD, where our stepping, callouts
//! and fuel column take over. Only the commander's own job is read, once,
//! with a User-Agent that says who we are.

use crate::state::AppState;
use ed_galaxy::router::{Hop, Route};
use tauri::State;

const USER_AGENT: &str = concat!("EDDA/", env!("CARGO_PKG_VERSION"), " (Elite Dangerous Desktop Aid)");

/// Fill in main-star classes for hops the index has as unknown, from
/// Spansh's documented `GET /system/{id64}` (one call per unknown hop,
/// sequential, remembered in `galaxy.star_overrides` and taught to the
/// index so later plots know too). Returns how many were resolved.
pub async fn resolve_unknown_stars(state: &AppState, hops: &mut [Hop]) -> usize {
    let Some(galaxy) = state.routing.galaxy(&state.data_dir) else { return 0 };
    let mut resolved = 0;
    // The community API first: it answers from its store at once and queues
    // what it lacks for EDSM, so one commander's route teaches everyone.
    if let Some(api) = crate::exchange::endpoint(state) {
        resolved += resolve_via_api(state, &galaxy, &api, hops).await;
    }
    for h in hops.iter_mut() {
        if h.class != ed_galaxy::StarClass::Unknown || h.id64 == 0 {
            continue;
        }
        let url = format!("https://spansh.co.uk/api/system/{}", h.id64);
        let Ok(resp) = state.http.get(&url).header(reqwest::header::USER_AGENT, USER_AGENT).send().await else { continue };
        let Ok(v) = resp.json::<serde_json::Value>().await else { continue };
        let rec = v.get("record").unwrap_or(&v);
        let Some(bodies) = rec.get("bodies").and_then(|b| b.as_array()) else { continue };
        let star = bodies
            .iter()
            .filter(|b| b.get("type").and_then(|t| t.as_str()) == Some("Star"))
            .max_by_key(|b| b.get("is_main_star").and_then(|m| m.as_bool()).unwrap_or(false));
        let Some(subtype) = star.and_then(|b| b.get("subtype")).and_then(|s| s.as_str()) else { continue };
        let class = ed_galaxy::StarClass::from_subtype(subtype);
        if class == ed_galaxy::StarClass::Unknown {
            continue;
        }
        h.class = class;
        h.scoopable = class.scoopable();
        galaxy.learn_class(h.id64, class);
        let star = star_override(h.id64, Some(h.name.clone()), subtype, class, "spansh /system");
        let _ = state.with_store(|s| ed_store::stars::save(s.conn(), &star).map_err(|e| e.to_string()));
        resolved += 1;
    }
    if resolved > 0 {
        tracing::info!(resolved, "learned main-star classes from Spansh");
    }
    resolved
}

/// Teach the index every override stored in the galaxy database.
/// Record a star class for a system from any source ("journal" beats
/// community data). The galaxy index only takes it for stars it has as
/// unknown; the overrides table keeps it regardless.
pub fn learn_star(state: &AppState, id64: u64, name: &str, subtype: &str, source: &str) {
    let _ = state.with_store(|s| {
        learn_star_conn(state, s.conn(), id64, name, subtype, source);
        Ok::<(), String>(())
    });
}

/// Same, writing through a connection the caller already holds. The
/// journal watcher calls this from inside its store lock: taking the
/// store again from there deadlocked the sync thread on the first scan
/// of an unknown star (Stop, Clear in game and every other store user
/// then hung behind it).
pub fn learn_star_conn(state: &AppState, conn: &rusqlite::Connection, id64: u64, name: &str, subtype: &str, source: &str) {
    let class = if source == "journal" { ed_galaxy::StarClass::from_journal(subtype) } else { ed_galaxy::StarClass::from_subtype(subtype) };
    if class == ed_galaxy::StarClass::Unknown {
        return;
    }
    if let Some(g) = state.routing.galaxy(&state.data_dir) {
        g.learn_class(id64, class);
    }
    let _ = store_star_override(conn, id64, name, subtype, class, source);
}

/// The store's record of a learned star. The `class` column holds
/// [`StarClass::name`] -- the key [`load_star_overrides_into`] reads back
/// and `knowledge_status` counts -- while `subtype` keeps the raw string
/// (a Spansh subtype or a journal letter) for reference only.
/// What `GET /v1/stars` answers.
#[derive(serde::Deserialize)]
struct StarsAnswer {
    #[serde(default)]
    known: Vec<KnownStar>,
}

#[derive(serde::Deserialize)]
struct KnownStar {
    id64: i64,
    class: u8,
    scoopable: bool,
}

/// Ask the community API for the hops' unknown main stars, in batches of
/// 500; apply and remember what it knows. Returns how many it resolved.
async fn resolve_via_api(state: &AppState, galaxy: &ed_galaxy::Galaxy, api: &str, hops: &mut [Hop]) -> usize {
    use ed_galaxy::star::StarClassCode as _;
    let unknown: Vec<u64> = hops.iter().filter(|h| h.class == ed_galaxy::StarClass::Unknown && h.id64 != 0).map(|h| h.id64).collect();
    let mut resolved = 0;
    for chunk in unknown.chunks(500) {
        let ids = chunk.iter().map(|id| id.to_string()).collect::<Vec<_>>().join(",");
        let url = format!("{api}/v1/stars?ids={ids}");
        let answer: StarsAnswer = match state.http.get(&url).timeout(std::time::Duration::from_secs(20)).send().await.and_then(|r| r.error_for_status()) {
            Ok(response) => match response.json().await {
                Ok(a) => a,
                Err(e) => {
                    tracing::warn!(error = %e, "community API stars answer unreadable");
                    return resolved;
                }
            },
            Err(e) => {
                tracing::warn!(error = %e, "community API stars lookup failed");
                return resolved;
            }
        };
        for known in answer.known {
            let class = ed_galaxy::StarClass::from_code(known.class);
            if class == ed_galaxy::StarClass::Unknown {
                continue;
            }
            for h in hops.iter_mut().filter(|h| h.id64 == known.id64 as u64 && h.class == ed_galaxy::StarClass::Unknown) {
                h.class = class;
                h.scoopable = known.scoopable;
                galaxy.learn_class(h.id64, class);
                let star = star_override(h.id64, Some(h.name.clone()), class.name(), class, "server");
                let _ = state.with_store(|s| ed_store::stars::save(s.conn(), &star).map_err(|e| e.to_string()));
                resolved += 1;
            }
        }
    }
    if resolved > 0 {
        tracing::info!(resolved, "main-star classes from the community API");
    }
    resolved
}

/// Remember a learned class in `star_overrides` (see [`star_override`]).
pub fn store_star_override(
    conn: &rusqlite::Connection,
    id64: u64,
    name: &str,
    subtype: &str,
    class: ed_galaxy::StarClass,
    source: &str,
) -> anyhow::Result<()> {
    ed_store::stars::save(conn, &star_override(id64, Some(name.to_string()), subtype, class, source))
}

fn star_override(id64: u64, name: Option<String>, subtype: &str, class: ed_galaxy::StarClass, source: &str) -> ed_store::stars::StarOverride {
    ed_store::stars::StarOverride {
        id64: id64 as i64,
        name,
        subtype: subtype.to_string(),
        class: class.name().to_string(),
        scoopable: class.scoopable(),
        source: source.to_string(),
    }
}

/// Every primary star the commander has ever scanned, into the overrides.
/// Runs once at startup; a few thousand rows.
/// Teach the galaxy index every primary star the journal has scanned.
/// Runs once at startup as a supervised blocking job.
pub fn backfill_journal_stars(state: &AppState) {
    {
        let rows: Vec<(u64, String, String)> = state.with_read(|s| {
            let mut stmt = match s.conn().prepare("SELECT raw FROM events WHERE event = 'Scan' AND raw LIKE '%\"StarType\"%'") {
                Ok(st) => st,
                Err(_) => return Vec::new(),
            };
            stmt.query_map([], |r| r.get::<_, String>(0))
                .map(|it| {
                    it.flatten()
                        .filter_map(|raw| serde_json::from_str::<serde_json::Value>(&raw).ok())
                        .filter(|v| v.get("DistanceFromArrivalLS").and_then(|d| d.as_f64()).unwrap_or(1e9) < 1.0)
                        .filter_map(|v| Some((v.get("SystemAddress")?.as_u64()?, v.get("StarSystem")?.as_str()?.to_string(), v.get("StarType")?.as_str()?.to_string())))
                        .collect()
                })
                .unwrap_or_default()
        });
        let n = rows.len();
        for (id64, name, t) in rows {
            learn_star(state, id64, &name, &t, "journal");
        }
        tracing::info!(stars = n, "journal star classes learned");
    }
}

pub fn load_star_overrides(state: &AppState) {
    let Some(galaxy) = state.routing.galaxy(&state.data_dir) else { return };
    state.with_read(|s| load_star_overrides_into(&galaxy, s.conn()));
}

/// Teach `galaxy` every row of `star_overrides`. Returns how many it took.
///
/// The `class` column is the key: it was written from the enum whatever
/// the source, so a journal "N" and a Spansh "Neutron Star" both read
/// back as Neutron. (Reading `subtype` here, as this once did, threw
/// away every journal-learned neutron and white dwarf on each launch,
/// because journal letters are not Spansh subtypes.) The subtype is only
/// a fallback for a row whose class this build cannot name.
pub fn load_star_overrides_into(galaxy: &ed_galaxy::Galaxy, conn: &rusqlite::Connection) -> usize {
    let rows = ed_store::stars::load_all(conn).unwrap_or_default();
    let mut learned = 0;
    for (id64, class, subtype) in rows {
        let class = ed_galaxy::StarClass::from_name(&class)
            .unwrap_or_else(|| ed_galaxy::StarClass::from_subtype(subtype.as_deref().unwrap_or("")));
        if class != ed_galaxy::StarClass::Unknown {
            galaxy.learn_class(id64 as u64, class);
            learned += 1;
        }
    }
    learned
}

#[cfg(test)]
mod override_tests {
    use super::*;

    fn galaxy_with_unknown_stars() -> (tempfile::TempDir, ed_galaxy::Galaxy) {
        let dir = tempfile::tempdir().unwrap();
        let dump = br#"[
{"id64":42,"name":"Scanned Neutron","coords":{"x":100,"y":0,"z":0},"bodies":[]},
{"id64":43,"name":"Scanned Dwarf","coords":{"x":120,"y":0,"z":0},"bodies":[]},
{"id64":44,"name":"Scanned Tauri","coords":{"x":140,"y":0,"z":0},"bodies":[]}
]"#;
        ed_galaxy::import::import_reader(Box::new(std::io::Cursor::new(dump)), dir.path(), &mut |_| {}).unwrap();
        let galaxy = ed_galaxy::Galaxy::open(dir.path()).unwrap();
        (dir, galaxy)
    }

    fn store() -> rusqlite::Connection {
        let conn = rusqlite::Connection::open_in_memory().unwrap();
        ed_store::schema::migrate(&conn).unwrap();
        ed_store::schema::attach_galaxy(&conn, None).unwrap();
        conn
    }

    fn class_of(g: &ed_galaxy::Galaxy, name: &str) -> ed_galaxy::StarClass {
        let idx = g.find(name).unwrap();
        g.class(&g.record(idx))
    }

    /// A neutron the commander scanned is written with the journal letter
    /// "N". On the next launch the override table is the only memory of
    /// it, so reading it back must yield Neutron -- not Unknown because
    /// "N" is not a Spansh subtype.
    #[test]
    fn journal_learned_stars_survive_a_restart() {
        let (_dir, galaxy) = galaxy_with_unknown_stars();
        let conn = store();
        store_star_override(&conn, 42, "Scanned Neutron", "N", ed_galaxy::StarClass::from_journal("N"), "journal").unwrap();
        store_star_override(&conn, 43, "Scanned Dwarf", "DA", ed_galaxy::StarClass::from_journal("DA"), "journal").unwrap();
        store_star_override(&conn, 44, "Scanned Tauri", "TTS", ed_galaxy::StarClass::from_journal("TTS"), "journal").unwrap();
        assert_eq!(class_of(&galaxy, "Scanned Neutron"), ed_galaxy::StarClass::Unknown, "nothing learned yet");

        let learned = load_star_overrides_into(&galaxy, &conn);
        assert_eq!(learned, 3);
        assert_eq!(class_of(&galaxy, "Scanned Neutron"), ed_galaxy::StarClass::Neutron);
        assert_eq!(class_of(&galaxy, "Scanned Dwarf"), ed_galaxy::StarClass::WhiteDwarf);
        assert_eq!(class_of(&galaxy, "Scanned Tauri"), ed_galaxy::StarClass::Proto);
    }

    /// The class column is what `knowledge_status` counts: it must hold
    /// the stable enum name, whatever the source string looked like.
    #[test]
    fn override_rows_persist_the_enum_name() {
        let conn = store();
        store_star_override(&conn, 42, "Scanned Neutron", "N", ed_galaxy::StarClass::from_journal("N"), "journal").unwrap();
        store_star_override(&conn, 43, "Spansh Neutron", "Neutron Star", ed_galaxy::StarClass::from_subtype("Neutron Star"), "spansh /system").unwrap();
        let neutrons: i64 = conn.query_row("SELECT count(*) FROM star_overrides WHERE class = 'Neutron'", [], |r| r.get(0)).unwrap();
        assert_eq!(neutrons, 2);
        let scoopable: i64 = conn.query_row("SELECT scoopable FROM star_overrides WHERE id64 = 42", [], |r| r.get(0)).unwrap();
        assert_eq!(scoopable, 0);
    }

    /// Rows written before the class column was read back may carry a
    /// class the current enum does not know; the subtype is still there.
    #[test]
    fn unreadable_class_falls_back_to_the_subtype() {
        let (_dir, galaxy) = galaxy_with_unknown_stars();
        let conn = store();
        conn.execute(
            "INSERT INTO star_overrides (id64, name, subtype, class, scoopable, source) VALUES (42, 'Scanned Neutron', 'Neutron Star', 'not-a-class', 0, 'edsm')",
            [],
        )
        .unwrap();
        assert_eq!(load_star_overrides_into(&galaxy, &conn), 1);
        assert_eq!(class_of(&galaxy, "Scanned Neutron"), ed_galaxy::StarClass::Neutron);
    }
}

/// Accepts a results URL (`.../results/<job>`) or a bare job id.
fn job_id(input: &str) -> Option<String> {
    let s = input.trim();
    let tail = s.rsplit('/').next().unwrap_or(s);
    let tail = tail.split(['?', '#']).next().unwrap_or(tail);
    let ok = tail.len() >= 32 && tail.chars().all(|c| c.is_ascii_hexdigit() || c == '-');
    ok.then(|| tail.to_uppercase())
}

#[tauri::command]
pub async fn import_spansh_route(state: State<'_, AppState>, link: String) -> Result<Route, String> {
    let job = job_id(&link).ok_or("that doesn't look like a Spansh results link or job id")?;
    let url = format!("https://spansh.co.uk/api/results/{job}");
    let resp = state
        .http
        .get(&url)
        .header(reqwest::header::USER_AGENT, USER_AGENT)
        .send()
        .await
        .map_err(|e| e.to_string())?
        .error_for_status()
        .map_err(|e| e.to_string())?;
    let v: serde_json::Value = resp.json().await.map_err(|e| e.to_string())?;
    let status = v.get("status").and_then(|s| s.as_str()).unwrap_or("");
    if status != "ok" {
        return Err(format!("Spansh job is {status:?} ({}) -- wait for it to finish and try again", v.get("state").and_then(|s| s.as_str()).unwrap_or("?")));
    }
    let result = v.get("result").ok_or("no result in the Spansh response")?;
    // Neutron/galaxy plotters use `jumps`; the trade/other planners differ.
    let jumps = result
        .get("jumps")
        .or_else(|| result.get("system_jumps"))
        .and_then(|j| j.as_array())
        .ok_or("this Spansh job is not a route (no jumps)")?;

    let galaxy = state.routing.galaxy(&state.data_dir);
    let f = |j: &serde_json::Value, k: &str| j.get(k).and_then(|x| x.as_f64()).map(|x| x as f32);
    let b = |j: &serde_json::Value, k: &str| j.get(k).and_then(|x| x.as_bool()).unwrap_or(false);

    let mut hops: Vec<Hop> = Vec::with_capacity(jumps.len());
    let mut total = 0.0f32;
    let mut boosted_jumps = 0;
    let mut refuel_stops = 0;
    let mut prev_neutron = false;
    let mut range_ly = 0.0f32;
    for j in jumps {
        let name = j.get("name").and_then(|n| n.as_str()).unwrap_or("?").to_string();
        let pos = [f(j, "x").unwrap_or(0.0), f(j, "y").unwrap_or(0.0), f(j, "z").unwrap_or(0.0)];
        let d = f(j, "distance").or_else(|| f(j, "distance_jumped")).unwrap_or(0.0);
        // Star class from our index when the system is known; else from
        // Spansh's flags.
        let (mut class, idx, id64) = galaxy
            .as_ref()
            .and_then(|g| g.find(&name).map(|i| (i, g.record(i))))
            .map(|(i, r)| (galaxy.as_ref().unwrap().class(&r), i, r.id64))
            .unwrap_or((ed_galaxy::StarClass::Unknown, u32::MAX, j.get("id64").and_then(|x| x.as_u64()).unwrap_or(0)));
        // Where the index has no star, Spansh's own flags are the best
        // information there is (its /system record is often empty for
        // these too). A neutron flag is a class; a scoopable flag is not.
        if class == ed_galaxy::StarClass::Unknown && b(j, "has_neutron") {
            class = ed_galaxy::StarClass::Neutron;
            if let Some(g) = galaxy.as_ref() {
                if id64 != 0 {
                    g.learn_class(id64, class);
                    // A route flag is weak evidence: never overwrite a real scan.
                    let star = star_override(id64, Some(name.clone()), "Neutron Star", class, "spansh route flag");
                    let _ = state.with_store(|s| {
                        ed_store::stars::save_if_unknown(s.conn(), &star).map_err(|e| e.to_string())
                    });
                }
            }
        }
        let boosted = prev_neutron && d > 0.0;
        let refuel = b(j, "must_refuel");
        if boosted {
            boosted_jumps += 1;
        }
        if refuel {
            refuel_stops += 1;
        }
        if !boosted && d > range_ly {
            range_ly = d;
        }
        total += d;
        hops.push(Hop {
            idx,
            id64,
            name,
            pos,
            class,
            scoopable: b(j, "is_scoopable") || class.scoopable(),
            distance_ly: d,
            boosted,
            total_ly: total,
            fuel_after: f(j, "fuel_in_tank"),
            refuel,
            fuel_optional: false,
            injection: None,
            synthesized: false,
            via_secondary: None,
        });
        prev_neutron = b(j, "has_neutron");
    }
    let straight = match (hops.first(), hops.last()) {
        (Some(a), Some(z)) => ed_galaxy::format::dist(a.pos, z.pos),
        _ => 0.0,
    };
    // Stars our index doesn't know: ask Spansh once, remember forever.
    load_star_overrides(&state);
    let learned = resolve_unknown_stars(&state, &mut hops).await;
    tracing::info!(job = %job, jumps = hops.len().saturating_sub(1), boosted = boosted_jumps, refuel = refuel_stops, learned, "imported Spansh route");
    Ok(Route {
        variants_run: 0,
        variants_finished: 0,
        ship_has_scoop: None,
        fsd_integrity: None,
        integrity_loss_per_boost: None,
        ship_has_afmu: None,
        ship_id: None,
        ship: None,
        range_ly,
        jumps: hops.len().saturating_sub(1),
        total_ly: total,
        straight_ly: straight,
        boosted_jumps,
        expansions: 0,
        elapsed_ms: 0,
        refuel_stops,
        injections: 0,
        secondary_boosts: 0,
        hops,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn job_ids_are_extracted_from_links() {
        assert_eq!(job_id("https://spansh.co.uk/exact-plotter/results/12345678-ABCD-11F1-9A6A-000000000000").as_deref(), Some("12345678-ABCD-11F1-9A6A-000000000000"));
        assert_eq!(job_id("12345678-abcd-11f1-9a6a-000000000000").as_deref(), Some("12345678-ABCD-11F1-9A6A-000000000000"));
        assert_eq!(job_id("https://spansh.co.uk/plotter"), None);
    }
}
