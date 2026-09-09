//! The EDSM proxy (maintainer-scheduled 2026-09-05, design ledgered
//! 2026-09-06): clients ask US about space, we answer from our own
//! galaxy + stars knowledge, and only a provably-stale cell costs ONE
//! upstream EDSM fetch for the whole fleet — fewer ultimate fetches.
//!
//! `GET /v1/knowledge/sphere?x&y&z&radius` mirrors EDSM's
//! `api-v1/sphere-systems` (same params, same response shape:
//! `[{name, id64, primaryStar: {type}}]`) so the client swap is a URL
//! change. `GET /v1/knowledge/bodies?systemName=` mirrors
//! `api-system-v1/bodies`, served verbatim from a long-TTL cache.
//!
//! Surveillance law: nothing here records who asked. The sweep table
//! stores cells and counts; handlers log outcome and duration, never
//! coordinates or system names.

use std::collections::HashSet;
use std::sync::Mutex;
use std::time::Duration;

use anyhow::{Context, Result};
use ed_galaxy::StarClass;
use sqlx::PgPool;

use crate::stars::{parse_edsm_time, StarObservation};

/// The client's cell grid (knowledge.rs CELL_LY) mirrored exactly: both
/// sides must agree on what "this region was swept" means.
pub const CELL_LY: f32 = 100.0;
/// A cell swept this recently is served entirely from our own data.
pub const REVISIT_DAYS: i64 = 30;
/// Bodies never change; the cache TTL is generous.
pub const BODIES_REVISIT_DAYS: i64 = 180;
/// The most systems a sphere answer carries. The client only sweeps
/// deserts (its local pre-check skips anywhere with < 25 unknowns), so
/// a truncated bubble query means someone is exploring the API, not a
/// commander losing data.
pub const MAX_SPHERE_SYSTEMS: usize = 5_000;
/// Fleet-wide outbound pacing toward EDSM: one call in flight, spaced
/// like the old per-client sweep (1.2 s), now for everyone combined.
pub const EDSM_SPACING: Duration = Duration::from_millis(1_200);
const EDSM_TIMEOUT: Duration = Duration::from_secs(30);
/// How long a coalesced waiter will sit out a leader's fetch before
/// giving up and serving what we already know.
const WAIT_BUDGET: Duration = Duration::from_secs(35);

pub fn cell_key(pos: [f32; 3]) -> String {
    let c = |v: f32| (v / CELL_LY).floor() as i64;
    format!("{}:{}:{}", c(pos[0]), c(pos[1]), c(pos[2]))
}

/// The canonical subtype string for a class — every value round-trips
/// through [`StarClass::from_subtype`] (pinned by test), so a client
/// parsing our answer with EDSM's parser lands on the same class.
pub fn canonical_subtype(class: StarClass) -> Option<&'static str> {
    use StarClass::*;
    Some(match class {
        Unknown => return None,
        O => "O Star",
        B => "B Star",
        A => "A Star",
        F => "F Star",
        G => "G Star",
        K => "K Star",
        M => "M Star",
        L => "L Star",
        T => "T Star",
        Y => "Y Star",
        Proto => "T Tauri Star",
        Exotic => "Wolf-Rayet Star",
        WhiteDwarf => "White Dwarf (D) Star",
        Neutron => "Neutron Star",
        BlackHole => "Black Hole",
    })
}

/// Single-flight per key: the first caller becomes the leader (gets a
/// guard) and everyone else waits for the guard to drop. The guard
/// releases on EVERY path — success, error, timeout, panic — via `Drop`,
/// the ledgered gotcha: a marker that only cleared on success would
/// strand every future waiter behind one hung EDSM call.
#[derive(Default)]
pub struct SingleFlight {
    inflight: Mutex<HashSet<String>>,
    done: tokio::sync::Notify,
}

pub struct FlightGuard<'a> {
    flight: &'a SingleFlight,
    key: String,
}

impl Drop for FlightGuard<'_> {
    fn drop(&mut self) {
        self.flight.inflight.lock().unwrap_or_else(|e| e.into_inner()).remove(&self.key);
        self.flight.done.notify_waiters();
    }
}

impl SingleFlight {
    /// `Some(guard)` = you are the leader; `None` = someone is already
    /// fetching this key.
    pub fn begin(&self, key: &str) -> Option<FlightGuard<'_>> {
        let mut inflight = self.inflight.lock().unwrap_or_else(|e| e.into_inner());
        if inflight.insert(key.to_owned()) {
            Some(FlightGuard { flight: self, key: key.to_owned() })
        } else {
            None
        }
    }

    /// Wait until `key` is no longer in flight (or the budget runs out —
    /// then the caller serves what it has rather than blocking forever).
    pub async fn wait(&self, key: &str) {
        let deadline = tokio::time::Instant::now() + WAIT_BUDGET;
        loop {
            let notified = self.done.notified();
            if !self.inflight.lock().unwrap_or_else(|e| e.into_inner()).contains(key) {
                return;
            }
            if tokio::time::timeout_at(deadline, notified).await.is_err() {
                return;
            }
        }
    }
}

/// Serializes and spaces every outbound EDSM call the process makes:
/// concurrency one, [`EDSM_SPACING`] between completions — the whole
/// fleet is exactly as polite as one pre-proxy client used to be.
pub struct Pacer {
    last: tokio::sync::Mutex<Option<tokio::time::Instant>>,
}

impl Default for Pacer {
    fn default() -> Self {
        Pacer { last: tokio::sync::Mutex::new(None) }
    }
}

impl Pacer {
    pub async fn permit(&self) -> PacerPermit<'_> {
        let guard = self.last.lock().await;
        if let Some(last) = *guard {
            tokio::time::sleep_until(last + EDSM_SPACING).await;
        }
        PacerPermit { guard }
    }
}

/// Held across the upstream call; stamps completion time on drop.
pub struct PacerPermit<'a> {
    guard: tokio::sync::MutexGuard<'a, Option<tokio::time::Instant>>,
}

impl Drop for PacerPermit<'_> {
    fn drop(&mut self) {
        *self.guard = Some(tokio::time::Instant::now());
    }
}

/// Is this cell's sweep fresh enough to answer from our own data?
pub async fn sweep_fresh(pool: &PgPool, cell: &str) -> Result<bool> {
    let row: Option<(bool,)> = sqlx::query_as(
        // make_interval takes int4; an i64 bind resolves to bigint and
        // the function lookup fails at runtime.
        "SELECT fetched_at > now() - make_interval(days => $2) FROM knowledge_sweeps WHERE cell = $1",
    )
    .bind(cell)
    .bind(REVISIT_DAYS as i32)
    .fetch_optional(pool)
    .await?;
    Ok(row.is_some_and(|(fresh,)| fresh))
}

/// Fetch one EDSM sphere and fold what it teaches into the stars table;
/// returns (systems seen, observations applied). The caller holds the
/// single-flight guard and the pacer permit.
pub async fn fetch_and_learn_sphere(
    pool: &PgPool,
    http: &reqwest::Client,
    pos: [f32; 3],
    radius: f32,
    cell: &str,
) -> Result<(usize, u64)> {
    let url = format!(
        "https://www.edsm.net/api-v1/sphere-systems?x={:.2}&y={:.2}&z={:.2}&radius={:.0}&showPrimaryStar=1&showId=1",
        pos[0], pos[1], pos[2], radius
    );
    let started = std::time::Instant::now();
    let response = http
        .get(&url)
        .header(reqwest::header::USER_AGENT, "EDDA-API/0.1 (edda community server)")
        .timeout(EDSM_TIMEOUT)
        .send()
        .await
        .context("EDSM sphere request")?
        .error_for_status()
        .context("EDSM sphere status")?;
    let systems: Vec<serde_json::Value> = response.json().await.context("EDSM sphere body")?;
    metrics::histogram!("edda_edsm_proxy_seconds", "endpoint" => "sphere")
        .record(started.elapsed().as_secs_f64());
    let now = chrono_free_now();
    let observations: Vec<StarObservation> = systems
        .iter()
        .filter_map(|s| {
            let address = i64::try_from(s.get("id64")?.as_u64()?).ok()?;
            let subtype = s.get("primaryStar")?.get("type")?.as_str()?.to_string();
            let class = StarClass::from_subtype(&subtype);
            if address <= 0 || class == StarClass::Unknown {
                return None;
            }
            Some(StarObservation {
                address,
                scoopable: class.scoopable(),
                subtype,
                class,
                observed_at: now,
                source: "edsm".to_string(),
            })
        })
        .collect();
    let learned = crate::stars::apply_stars(pool, &observations).await?;
    sqlx::query(
        "INSERT INTO knowledge_sweeps (cell, fetched_at, systems, learned) \
         VALUES ($1, now(), $2, $3) \
         ON CONFLICT (cell) DO UPDATE SET \
           fetched_at = now(), systems = EXCLUDED.systems, learned = EXCLUDED.learned",
    )
    .bind(cell)
    .bind(systems.len() as i32)
    .bind(learned as i32)
    .execute(pool)
    .await?;
    Ok((systems.len(), learned))
}

/// Seconds since the epoch without pulling a date crate into ed-api.
fn chrono_free_now() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0)
}

/// Answer a sphere from the resident galaxy plus the stars-table
/// overlay (the classes learned since the index was built — including
/// what other commanders' sweeps just taught us).
pub async fn answer_sphere(
    pool: &PgPool,
    galaxy: &ed_galaxy::Galaxy,
    pos: [f32; 3],
    radius: f32,
) -> Result<(Vec<serde_json::Value>, bool)> {
    struct Hit {
        id64: i64,
        name: String,
        class: StarClass,
        pos: [f32; 3],
        distance: f32,
    }
    let mut hits: Vec<Hit> = Vec::new();
    let mut truncated = false;
    galaxy.for_each_within(pos, radius, |idx, dist| {
        if hits.len() >= MAX_SPHERE_SYSTEMS {
            truncated = true;
            return;
        }
        let r = galaxy.record(idx);
        let Ok(id64) = i64::try_from(r.id64) else { return };
        hits.push(Hit {
            id64,
            name: galaxy.name(&r).to_string(),
            class: galaxy.class(&r),
            pos: galaxy.pos_of(idx),
            distance: dist,
        });
    });
    // Overlay: newer knowledge wins over the mapped index.
    let addresses: Vec<i64> = hits.iter().map(|h| h.id64).collect();
    let overlay: Vec<(i64, String)> =
        sqlx::query_as("SELECT address, subtype FROM stars WHERE address = ANY($1)")
            .bind(&addresses)
            .fetch_all(pool)
            .await?;
    let overlay: std::collections::HashMap<i64, String> = overlay.into_iter().collect();
    // The system row for each hit, so a sphere answer carries population
    // and Powerplay for the client's systems_near (2026-09-08).
    let facts: Vec<(i64, Option<i64>, Option<String>, Option<String>)> = sqlx::query_as(
        "SELECT address, population, controlling_power, power_state FROM systems WHERE address = ANY($1)",
    )
    .bind(&addresses)
    .fetch_all(pool)
    .await?;
    let facts: std::collections::HashMap<i64, (Option<i64>, Option<String>, Option<String>)> =
        facts.into_iter().map(|(a, p, cp, ps)| (a, (p, cp, ps))).collect();
    let answer = hits
        .into_iter()
        .filter_map(|h| {
            let subtype: String = match overlay.get(&h.id64) {
                Some(s) => s.clone(),
                None => canonical_subtype(h.class)?.to_string(),
            };
            // `coords` and `distance` as EDSM's sphere-systems writes them
            // (showCoordinates=1): the client's fuel-trap guard reads the
            // sphere when the bundled bubble does not hold the target
            // (API-only spec, Phase B.2) and needs positions to find the
            // nearest scoopable star.
            let (population, controlling_power, power_state) = facts.get(&h.id64).cloned().unwrap_or((None, None, None));
            Some(serde_json::json!({
                "name": h.name,
                "id64": h.id64,
                "coords": { "x": h.pos[0], "y": h.pos[1], "z": h.pos[2] },
                "distance": h.distance,
                "primaryStar": { "type": subtype },
                "population": population,
                "controllingPower": controlling_power,
                "powerState": power_state,
            }))
        })
        .collect();
    Ok((answer, truncated))
}

/// The cached-or-fetched EDSM bodies document for one system, verbatim.
pub async fn bodies_document(
    pool: &PgPool,
    http: &reqwest::Client,
    pacer: &Pacer,
    flight: &SingleFlight,
    system_name: &str,
) -> Result<Option<String>> {
    let cached: Option<(String,)> = sqlx::query_as(
        "SELECT body FROM knowledge_bodies \
         WHERE system_name = $1 AND fetched_at > now() - make_interval(days => $2)",
    )
    .bind(system_name)
    .bind(BODIES_REVISIT_DAYS as i32)
    .fetch_optional(pool)
    .await?;
    if let Some((body,)) = cached {
        metrics::counter!("edda_knowledge_requests_total", "endpoint" => "bodies", "outcome" => "cached")
            .increment(1);
        return Ok(Some(body));
    }
    let flight_key = format!("bodies:{system_name}");
    let Some(_guard) = flight.begin(&flight_key) else {
        flight.wait(&flight_key).await;
        let refreshed: Option<(String,)> =
            sqlx::query_as("SELECT body FROM knowledge_bodies WHERE system_name = $1")
                .bind(system_name)
                .fetch_optional(pool)
                .await?;
        metrics::counter!("edda_knowledge_requests_total", "endpoint" => "bodies", "outcome" => "coalesced")
            .increment(1);
        return Ok(refreshed.map(|(body,)| body));
    };
    let _permit = pacer.permit().await;
    let url = reqwest::Url::parse_with_params(
        "https://www.edsm.net/api-system-v1/bodies",
        [("systemName", system_name)],
    )
    .context("bodies url")?;
    let started = std::time::Instant::now();
    let body = http
        .get(url)
        .header(reqwest::header::USER_AGENT, "EDDA-API/0.1 (edda community server)")
        .timeout(EDSM_TIMEOUT)
        .send()
        .await
        .context("EDSM bodies request")?
        .error_for_status()
        .context("EDSM bodies status")?
        .text()
        .await
        .context("EDSM bodies body")?;
    metrics::histogram!("edda_edsm_proxy_seconds", "endpoint" => "bodies")
        .record(started.elapsed().as_secs_f64());
    // Refuse to cache something that is not a JSON object — an EDSM
    // error page cached for six months would be a long-lived lie.
    if serde_json::from_str::<serde_json::Map<String, serde_json::Value>>(&body).is_err() {
        anyhow::bail!("EDSM bodies response is not a JSON object");
    }
    sqlx::query(
        "INSERT INTO knowledge_bodies (system_name, fetched_at, body) VALUES ($1, now(), $2) \
         ON CONFLICT (system_name) DO UPDATE SET fetched_at = now(), body = EXCLUDED.body",
    )
    .bind(system_name)
    .bind(&body)
    .execute(pool)
    .await?;
    metrics::counter!("edda_knowledge_requests_total", "endpoint" => "bodies", "outcome" => "fetched")
        .increment(1);
    Ok(Some(body))
}

// parse_edsm_time is re-exported through stars; referenced here so the
// import is exercised even when the sphere path alone is compiled in
// tests.
#[allow(dead_code)]
fn _uses(t: &str) -> Option<i64> {
    parse_edsm_time(t)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Every canonical subtype must land back on its own class through
    /// the same parser the client uses — otherwise our answers would
    /// teach clients the wrong stars.
    #[test]
    fn canonical_subtypes_round_trip() {
        for class in StarClass::ALL {
            match canonical_subtype(class) {
                None => assert_eq!(class, StarClass::Unknown),
                Some(subtype) => assert_eq!(StarClass::from_subtype(subtype), class, "{subtype}"),
            }
        }
    }

    /// The grid must match the client's floor-division exactly,
    /// negatives included.
    #[test]
    fn cell_key_matches_the_client_grid() {
        assert_eq!(cell_key([0.0, 0.0, 0.0]), "0:0:0");
        assert_eq!(cell_key([99.9, 100.0, 250.0]), "0:1:2");
        assert_eq!(cell_key([-0.1, -100.0, -100.1]), "-1:-1:-2");
    }

    #[test]
    fn single_flight_leader_guard_clears_on_drop() {
        let flight = SingleFlight::default();
        let guard = flight.begin("cell").expect("first caller leads");
        assert!(flight.begin("cell").is_none(), "second caller coalesces");
        assert!(flight.begin("other").is_some(), "keys are independent");
        drop(guard);
        assert!(flight.begin("cell").is_some(), "guard drop released the key");
    }

    #[tokio::test]
    async fn single_flight_wait_returns_when_leader_drops() {
        let flight = std::sync::Arc::new(SingleFlight::default());
        let guard = flight.begin("k").unwrap();
        let waiter = {
            let flight = flight.clone();
            tokio::spawn(async move { flight.wait("k").await })
        };
        tokio::time::sleep(Duration::from_millis(50)).await;
        assert!(!waiter.is_finished(), "waiter blocks while the leader runs");
        drop(guard);
        tokio::time::timeout(Duration::from_secs(2), waiter)
            .await
            .expect("waiter released promptly")
            .unwrap();
    }
}

// ── By-name system lookup ────────────────────────────────────────────
//
// `GET /v1/knowledge/system?name=` is the by-name twin of the sphere
// proxy, added because the Galaxy tab was still calling edsm.net
// DIRECTLY from the commander's machine — not as a fallback, as its ONLY
// path — sending a system name to a third party from a client that had
// just been told "use local data" (maintainer, 2026-09-06: "route the EDSM
// calls through our API. This was supposed to be done.").
//
// The answer shape mirrors EDSM's `api-v1/system` with showCoordinates +
// showInformation + showPrimaryStar, because that is what the client
// already parses: the swap is a URL change, not a parser change.

/// One system as the Galaxy tab wants it. Field names and nesting follow
/// EDSM so the client's existing `EdsmSystemResponse` parses this
/// unchanged.
#[derive(Debug, serde::Serialize, PartialEq)]
pub struct SystemAnswer {
    pub name: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub id64: Option<i64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub coords: Option<serde_json::Value>,
    pub information: serde_json::Value,
    #[serde(rename = "primaryStar", skip_serializing_if = "Option::is_none")]
    pub primary_star: Option<serde_json::Value>,
}

/// Build the `information` object from what we hold, omitting nulls.
/// EDSM returns `[]` for a system it has nothing political about and the
/// client tolerates either; an empty OBJECT is the honest twin.
#[allow(clippy::too_many_arguments)]
fn information(
    allegiance: Option<String>,
    government: Option<String>,
    population: Option<i64>,
    security: Option<String>,
    economy: Option<String>,
    controlling_power: Option<String>,
    power_state: Option<String>,
    powers: Option<String>,
) -> serde_json::Value {
    let mut map = serde_json::Map::new();
    let mut put = |k: &str, v: Option<serde_json::Value>| {
        if let Some(v) = v {
            map.insert(k.to_string(), v);
        }
    };
    put("allegiance", allegiance.map(serde_json::Value::from));
    put("government", government.map(serde_json::Value::from));
    put("population", population.map(serde_json::Value::from));
    put("security", security.map(serde_json::Value::from));
    put("economy", economy.map(serde_json::Value::from));
    // Powerplay, so the API-only client's system lookup carries what the
    // local sys_systems row used to (2026-09-08).
    put("controllingPower", controlling_power.map(serde_json::Value::from));
    put("powerState", power_state.map(serde_json::Value::from));
    put(
        "powers",
        powers.map(|p| serde_json::Value::from(p.split(',').map(|x| x.trim().to_string()).filter(|x| !x.is_empty()).collect::<Vec<_>>())),
    );
    serde_json::Value::Object(map)
}

type LocalSystemRow = (
    i64,
    String,
    Option<f64>,
    Option<f64>,
    Option<f64>,
    Option<i64>,
    Option<String>,
    Option<String>,
    Option<String>,
    Option<String>,
    Option<String>,
    Option<bool>,
    Option<String>,
    Option<String>,
    Option<String>,
);

/// What our own tables know about `name`, or `None` if the name is new to
/// us. The primary star comes from the same `stars` table the sphere
/// answer reads, so the two endpoints never disagree about a system.
pub async fn local_system(pool: &PgPool, name: &str) -> Result<Option<SystemAnswer>> {
    let row: Option<LocalSystemRow> = sqlx::query_as(
        "SELECT s.address, s.name, s.x, s.y, s.z, s.population, s.security, \
                s.allegiance, s.government, s.economy, st.subtype, st.scoopable, \
                s.controlling_power, s.power_state, s.powers \
         FROM systems s LEFT JOIN stars st ON st.address = s.address \
         WHERE lower(s.name) = lower($1) AND s.address > 0",
    )
    .bind(name)
    .fetch_optional(pool)
    .await?;
    let Some((
        address,
        name,
        x,
        y,
        z,
        population,
        security,
        allegiance,
        government,
        economy,
        subtype,
        scoopable,
        controlling_power,
        power_state,
        powers,
    )) = row
    else {
        return Ok(None);
    };
    let coords = match (x, y, z) {
        (Some(x), Some(y), Some(z)) => Some(serde_json::json!({ "x": x, "y": y, "z": z })),
        _ => None,
    };
    let primary_star = subtype.map(|subtype| {
        serde_json::json!({ "type": subtype, "isScoopable": scoopable.unwrap_or(false) })
    });
    Ok(Some(SystemAnswer {
        name,
        id64: Some(address),
        coords,
        information: information(allegiance, government, population, security, economy, controlling_power, power_state, powers),
        primary_star,
    }))
}

/// Write an upstream answer into our own tables so the next commander to
/// ask is served locally. Best-effort by design: a system we cannot store
/// (EDSM gave no id64, a concurrent writer took the name) is still
/// ANSWERED — learning is the optimisation, the answer is the product.
pub async fn learn_system(pool: &PgPool, answer: &SystemAnswer) -> Result<()> {
    let Some(address) = answer.id64.filter(|a| *a > 0) else {
        return Ok(());
    };
    let info = &answer.information;
    let text = |k: &str| info.get(k).and_then(|v| v.as_str()).map(str::to_string);
    let coord = |k: &str| {
        answer.coords.as_ref().and_then(|c| c.get(k)).and_then(serde_json::Value::as_f64)
    };
    sqlx::query(
        "INSERT INTO systems (address, name, x, y, z, population, security, allegiance, \
                              government, economy, source_observed_at, provenance) \
         VALUES ($1,$2,$3,$4,$5,$6,$7,$8,$9,$10, now(), 'edsm') \
         ON CONFLICT (address) DO UPDATE SET \
           x = COALESCE(EXCLUDED.x, systems.x), \
           y = COALESCE(EXCLUDED.y, systems.y), \
           z = COALESCE(EXCLUDED.z, systems.z), \
           population = COALESCE(EXCLUDED.population, systems.population), \
           security = COALESCE(EXCLUDED.security, systems.security), \
           allegiance = COALESCE(EXCLUDED.allegiance, systems.allegiance), \
           government = COALESCE(EXCLUDED.government, systems.government), \
           economy = COALESCE(EXCLUDED.economy, systems.economy)",
    )
    .bind(address)
    .bind(&answer.name)
    .bind(coord("x"))
    .bind(coord("y"))
    .bind(coord("z"))
    .bind(info.get("population").and_then(serde_json::Value::as_i64))
    .bind(text("security"))
    .bind(text("allegiance"))
    .bind(text("government"))
    .bind(text("economy"))
    .execute(pool)
    .await?;
    // The primary star goes into the table the sphere proxy fills, so one
    // system never disagrees with itself across the two endpoints.
    if let Some(subtype) =
        answer.primary_star.as_ref().and_then(|s| s.get("type")).and_then(|v| v.as_str())
    {
        let class = StarClass::from_subtype(subtype);
        if class != StarClass::Unknown {
            crate::stars::apply_stars(
                pool,
                &[StarObservation {
                    address,
                    scoopable: class.scoopable(),
                    subtype: subtype.to_string(),
                    class,
                    observed_at: chrono_free_now(),
                    source: "edsm".to_string(),
                }],
            )
            .await?;
        }
    }
    Ok(())
}

/// Parse EDSM's `api-v1/system` reply. EDSM answers an unknown name with
/// `{}` (and `information` as `[]` when it has nothing political), so an
/// absent name is the UNKNOWN signal, not an error.
pub fn parse_upstream_system(value: &serde_json::Value) -> Option<SystemAnswer> {
    let name = value.get("name").and_then(|v| v.as_str()).filter(|n| !n.is_empty())?;
    let information = match value.get("information") {
        Some(v) if v.is_object() => v.clone(),
        _ => serde_json::Value::Object(serde_json::Map::new()),
    };
    Some(SystemAnswer {
        name: name.to_string(),
        id64: value.get("id64").and_then(serde_json::Value::as_i64),
        coords: value.get("coords").filter(|c| c.is_object()).cloned(),
        information,
        primary_star: value.get("primaryStar").filter(|s| s.is_object()).cloned(),
    })
}

/// The outcome of one by-name lookup, so the handler can count it.
pub enum SystemOutcome {
    Local(SystemAnswer),
    /// Known to the mapped routing index but not to `systems` (an
    /// uninhabited system): name, id64, coords and primary star, no
    /// politics — and no upstream call, because we already know it.
    Indexed(SystemAnswer),
    Fetched(SystemAnswer),
    Coalesced(Option<SystemAnswer>),
    Unknown,
}

/// Answer `name` from our own tables, going upstream ONCE for the whole
/// fleet when the name is new to us — the sphere proxy's bargain, by name.
pub async fn system_document(
    pool: &PgPool,
    http: &reqwest::Client,
    pacer: &Pacer,
    flight: &SingleFlight,
    galaxy: Option<&ed_galaxy::Galaxy>,
    name: &str,
) -> Result<SystemOutcome> {
    if let Some(answer) = local_system(pool, name).await? {
        return Ok(SystemOutcome::Local(answer));
    }
    // RULING (maintainer, 2026-09-07): the server consults everything it holds
    // before going upstream. The routing index mapped for /v1/route knows
    // every system the fleet's autocomplete can offer; a 404 for one of
    // those (field case 06:31Z) was this step missing.
    if let Some(answer) = galaxy.and_then(|g| index_system(g, name)) {
        return Ok(SystemOutcome::Indexed(answer));
    }
    let key = format!("system:{}", name.to_lowercase());
    let Some(_guard) = flight.begin(&key) else {
        flight.wait(&key).await;
        return Ok(SystemOutcome::Coalesced(local_system(pool, name).await?));
    };
    let _permit = pacer.permit().await;
    let url = reqwest::Url::parse_with_params(
        "https://www.edsm.net/api-v1/system",
        [
            ("systemName", name),
            ("showCoordinates", "1"),
            ("showInformation", "1"),
            ("showPrimaryStar", "1"),
            // The client never asked for this; the server must, or a
            // learned system has no primary key to be stored under.
            ("showId", "1"),
        ],
    )
    .context("system url")?;
    let started = std::time::Instant::now();
    let value: serde_json::Value = http
        .get(url)
        .header(reqwest::header::USER_AGENT, "EDDA-API/0.1 (edda community server)")
        .timeout(EDSM_TIMEOUT)
        .send()
        .await
        .context("EDSM system request")?
        .error_for_status()
        .context("EDSM system status")?
        .json()
        .await
        .context("EDSM system body")?;
    metrics::histogram!("edda_edsm_proxy_seconds", "endpoint" => "system")
        .record(started.elapsed().as_secs_f64());
    let Some(answer) = parse_upstream_system(&value) else {
        return Ok(SystemOutcome::Unknown);
    };
    // A learning failure must never cost the commander their answer.
    if let Err(error) = learn_system(pool, &answer).await {
        tracing::warn!(%error, "knowledge: system answered but not learned");
    }
    Ok(SystemOutcome::Fetched(answer))
}

/// Answer `name` from the mapped routing index: canonical name, id64,
/// coordinates and the primary star's class. `information` is the honest
/// empty object — the index carries no politics — so the client's parser
/// reads it exactly as it reads an EDSM answer for an unpopulated system.
pub fn index_system(galaxy: &ed_galaxy::Galaxy, name: &str) -> Option<SystemAnswer> {
    let idx = galaxy.find(name)?;
    let r = galaxy.record(idx);
    let primary_star = canonical_subtype(galaxy.class(&r)).map(|subtype| {
        serde_json::json!({ "type": subtype, "isScoopable": galaxy.scoopable(idx) })
    });
    Some(SystemAnswer {
        name: galaxy.name(&r).to_string(),
        id64: i64::try_from(r.id64).ok(),
        coords: Some(serde_json::json!({ "x": r.x, "y": r.y, "z": r.z })),
        information: serde_json::json!({}),
        primary_star,
    })
}

#[cfg(test)]
mod index_tests {
    use super::*;

    fn tiny_galaxy() -> (tempfile::TempDir, ed_galaxy::Galaxy) {
        let dir = tempfile::tempdir().unwrap();
        let source = r#"[
{"id64":10477373803,"name":"Sol","coords":{"x":0,"y":0,"z":0},"bodies":[{"type":"Star","subType":"G (White-Yellow) Star","mainStar":true}]},
{"id64":5031654888146,"name":"Wongi","coords":{"x":-12.5,"y":8.25,"z":-40},"bodies":[{"type":"Star","subType":"M (Red dwarf) Star","mainStar":true}]}
]"#;
        let path = dir.path().join("galaxy");
        ed_galaxy::import::import_reader(Box::new(source.as_bytes()), &path, &mut |_| {}).unwrap();
        let galaxy = ed_galaxy::Galaxy::open(&path).unwrap();
        (dir, galaxy)
    }

    #[test]
    fn a_system_the_index_knows_is_answered_without_going_upstream() {
        let (_dir, g) = tiny_galaxy();
        let answer = index_system(&g, "wongi").expect("the index knows Wongi");
        assert_eq!(answer.name, "Wongi", "canonical case from the index, not the query");
        assert_eq!(answer.id64, Some(5031654888146));
        assert_eq!(answer.coords, Some(serde_json::json!({ "x": -12.5, "y": 8.25, "z": -40.0 })));
        assert_eq!(answer.information, serde_json::json!({}));
        let star = answer.primary_star.expect("primary star from the index class");
        // canonical_subtype's class string, the one the client's parser
        // round-trips — not the import source's wording.
        assert_eq!(star["type"], "M Star");
        // KGBFOAM: a red dwarf scoops.
        assert_eq!(star["isScoopable"], true);
    }

    #[test]
    fn scoopable_flag_follows_the_class() {
        let (_dir, g) = tiny_galaxy();
        let sol = index_system(&g, "Sol").unwrap();
        assert_eq!(sol.primary_star.unwrap()["isScoopable"], true);
    }

    #[test]
    fn an_unknown_name_is_none_so_the_upstream_step_still_runs() {
        let (_dir, g) = tiny_galaxy();
        assert!(index_system(&g, "Nowhere Prime").is_none());
    }

    #[test]
    fn the_answer_serialises_like_a_local_or_fetched_one() {
        let (_dir, g) = tiny_galaxy();
        let v = serde_json::to_value(index_system(&g, "Sol").unwrap()).unwrap();
        assert_eq!(v["name"], "Sol");
        assert!(v["coords"].is_object());
        assert!(v["information"].is_object());
        assert_eq!(v["primaryStar"]["type"], "G Star");
    }
}
