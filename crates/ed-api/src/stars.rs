//! Main-star classes the service learns and publishes.
//!
//! Three feeds into one table, `stars`: the weekly EDSM `bodies7days`
//! dump (`hydrate --edsm-bodies`), EDSM lookups for systems clients asked
//! about (`GET /v1/stars?ids=` queues what the store cannot answer; the
//! server drains the queue against EDSM one request a second), and -- in
//! time -- commanders' own scans. `publish-stars` writes the table as the
//! `stars` product: one EBEX stars section, a few megabytes, refreshed
//! independently of the community baseline. Clients merge it into their
//! star overrides and the routing index learns the classes.

use std::io::{BufRead, BufReader};
use std::path::Path;

use anyhow::{Context, Result};
use ed_ebex::{SnapshotHeader, SnapshotWriter, StarRecord, SECTION_STARS, STAR_RECORD_BYTES};
use ed_galaxy::star::StarClassCode as _;
use ed_galaxy::StarClass;
use ed_sync::{ArtifactFile, Manifest, Product, ProductKey};
use serde::{Deserialize, Serialize};
use sqlx::PgPool;

/// One main star as a source reports it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StarObservation {
    pub address: i64,
    pub subtype: String,
    pub class: StarClass,
    pub scoopable: bool,
    pub observed_at: i64,
    pub source: String,
}

/// Parse one EDSM `bodies*.json` record (one JSON object per line) into a
/// main-star observation; `None` for planets, secondary stars, records
/// without a class the index knows, or malformed lines.
pub fn edsm_body_main_star(line: &str, source: &str) -> Option<StarObservation> {
    let t = line.trim().trim_end_matches(',');
    if !t.starts_with('{') {
        return None;
    }
    let v: serde_json::Value = serde_json::from_str(t).ok()?;
    if v.get("type")?.as_str()? != "Star" || !v.get("isMainStar")?.as_bool()? {
        return None;
    }
    let subtype = v.get("subType")?.as_str()?.to_string();
    let class = StarClass::from_subtype(&subtype);
    if class == StarClass::Unknown {
        return None;
    }
    let address = i64::try_from(v.get("systemId64")?.as_u64()?).ok()?;
    if address <= 0 {
        return None;
    }
    let observed_at = v
        .get("updateTime")
        .and_then(|s| s.as_str())
        .and_then(parse_edsm_time)
        .unwrap_or(0);
    Some(StarObservation {
        address,
        scoopable: v
            .get("isScoopable")
            .and_then(|b| b.as_bool())
            .unwrap_or_else(|| class.scoopable()),
        subtype,
        class,
        observed_at,
        source: source.to_string(),
    })
}

/// The main-star observation a Spansh galaxy-dump body carries, or `None`
/// for planets, secondary stars, and classes the index has no code for.
/// The stage-1c seam (item 47): the weekly `galaxy_7days` hydrate teaches
/// the stars table through this, and the nightly reconcile folds the
/// classes into the published routing index.
pub fn spansh_body_main_star(
    system_id64: i64,
    body: &ed_store::galaxy::spansh::Body,
    updated: Option<i64>,
    source: &str,
) -> Option<StarObservation> {
    if system_id64 <= 0 || !body.main_star || body.kind.as_deref() != Some("Star") {
        return None;
    }
    let subtype = body.sub_type.clone()?;
    let class = StarClass::from_subtype(&subtype);
    if class == StarClass::Unknown {
        return None;
    }
    Some(StarObservation {
        address: system_id64,
        scoopable: class.scoopable(),
        subtype,
        class,
        observed_at: updated.unwrap_or(0),
        source: source.to_string(),
    })
}

/// EDSM writes `YYYY-MM-DD HH:MM:SS` (UTC).
pub fn parse_edsm_time(s: &str) -> Option<i64> {
    let s = s.trim();
    if s.len() < 19 {
        return None;
    }
    let (date, time) = (&s[..10], &s[11..19]);
    let mut d = date.split('-').map(|p| p.parse::<i64>().ok());
    let mut t = time.split(':').map(|p| p.parse::<i64>().ok());
    let (y, mo, da) = (d.next()??, d.next()??, d.next()??);
    let (h, mi, se) = (t.next()??, t.next()??, t.next()??);
    // Days from civil (Howard Hinnant), valid for the years in question.
    let (y, mo) = if mo <= 2 { (y - 1, mo + 12) } else { (y, mo) };
    let era = y.div_euclid(400);
    let yoe = y - era * 400;
    let doy = (153 * (mo - 3) + 2) / 5 + da - 1;
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
    let days = era * 146_097 + doe - 719_468;
    Some(days * 86_400 + h * 3_600 + mi * 60 + se)
}

/// One observation per address, newest kept (stable on ties, so the
/// first-listed main star of a multi-main-star system wins). Postgres
/// refuses a batch that touches one conflict key twice — and Spansh
/// systems really do carry two `mainStar` bodies sometimes.
pub fn dedupe_newest(stars: &[StarObservation]) -> Vec<StarObservation> {
    let mut newest: std::collections::HashMap<i64, &StarObservation> =
        std::collections::HashMap::new();
    for star in stars {
        match newest.entry(star.address) {
            std::collections::hash_map::Entry::Vacant(slot) => {
                slot.insert(star);
            }
            std::collections::hash_map::Entry::Occupied(mut held) => {
                if star.observed_at > held.get().observed_at {
                    held.insert(star);
                }
            }
        }
    }
    let mut out: Vec<StarObservation> = newest.into_values().cloned().collect();
    out.sort_unstable_by_key(|star| star.address);
    out
}

/// Upsert observations, strictly newer wins (an equal or older
/// observation leaves the row alone). Duplicate addresses within the
/// batch collapse to the newest first. Returns rows written.
pub async fn apply_stars(pool: &PgPool, stars: &[StarObservation]) -> Result<u64> {
    if stars.is_empty() {
        return Ok(0);
    }
    let stars = dedupe_newest(stars);
    let stars = stars.as_slice();
    let addresses: Vec<i64> = stars.iter().map(|s| s.address).collect();
    let classes: Vec<i16> = stars.iter().map(|s| i16::from(s.class.code())).collect();
    let scoopable: Vec<bool> = stars.iter().map(|s| s.scoopable).collect();
    let subtypes: Vec<&str> = stars.iter().map(|s| s.subtype.as_str()).collect();
    let sources: Vec<&str> = stars.iter().map(|s| s.source.as_str()).collect();
    let observed: Vec<i64> = stars.iter().map(|s| s.observed_at).collect();
    let written = sqlx::query(
        "INSERT INTO stars (address, class, scoopable, subtype, source, observed_at) \
         SELECT address, class, scoopable, subtype, source, to_timestamp(observed_at) \
         FROM unnest($1::bigint[], $2::smallint[], $3::boolean[], $4::text[], $5::text[], $6::bigint[]) \
              AS t(address, class, scoopable, subtype, source, observed_at) \
         ON CONFLICT (address) DO UPDATE SET \
           class = EXCLUDED.class, scoopable = EXCLUDED.scoopable, subtype = EXCLUDED.subtype, \
           source = EXCLUDED.source, observed_at = EXCLUDED.observed_at \
         WHERE stars.observed_at < EXCLUDED.observed_at",
    )
    .bind(&addresses)
    .bind(&classes)
    .bind(&scoopable)
    .bind(&subtypes)
    .bind(&sources)
    .bind(&observed)
    .execute(pool)
    .await?
    .rows_affected();
    // Anything now known no longer needs a lookup.
    sqlx::query("DELETE FROM star_lookups WHERE address = ANY($1)")
        .bind(&addresses)
        .execute(pool)
        .await?;
    Ok(written)
}

#[derive(Debug, Serialize)]
pub struct EdsmBodiesResult {
    pub source: String,
    pub bodies: u64,
    pub main_stars: u64,
    pub written: u64,
}

/// Stream an EDSM bodies dump (`bodies7days.json.gz` or uncompressed) into
/// `stars`, recorded as a hydration job. Memory is one batch.
pub async fn hydrate_edsm_bodies(pool: &PgPool, path: &Path) -> Result<EdsmBodiesResult> {
    let source = format!(
        "edsm:bodies:{}",
        path.file_name()
            .map(|n| n.to_string_lossy().into_owned())
            .unwrap_or_default()
    );
    let metadata =
        std::fs::metadata(path).with_context(|| format!("reading {}", path.display()))?;
    let modified = metadata
        .modified()
        .ok()
        .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
        .map_or(0, |d| d.as_secs() as i64);
    let run_id: i64 = sqlx::query_scalar(
        "INSERT INTO service_hydrations (source, source_observed_at, source_bytes, status) \
         VALUES ($1, to_timestamp($2), $3, 'running') RETURNING id",
    )
    .bind(&source)
    .bind(modified)
    .bind(i64::try_from(metadata.len())?)
    .fetch_one(pool)
    .await?;

    let file = std::fs::File::open(path)?;
    let reader: Box<dyn std::io::Read + Send> = if path.extension().is_some_and(|e| e == "gz") {
        Box::new(flate2::read::GzDecoder::new(file))
    } else {
        Box::new(file)
    };
    let (tx, mut rx) = tokio::sync::mpsc::channel::<Vec<StarObservation>>(4);
    let source_for_reader = source.clone();
    let reader_task = tokio::task::spawn_blocking(move || -> Result<(u64, u64)> {
        let mut bodies = 0u64;
        let mut main_stars = 0u64;
        let mut batch = Vec::with_capacity(5_000);
        for line in BufReader::with_capacity(1 << 20, reader).lines() {
            let line = line?;
            if !line.trim_start().starts_with('{') {
                continue;
            }
            bodies += 1;
            if let Some(star) = edsm_body_main_star(&line, &source_for_reader) {
                main_stars += 1;
                batch.push(star);
                if batch.len() == 5_000 {
                    tx.blocking_send(std::mem::take(&mut batch))
                        .map_err(|_| anyhow::anyhow!("writer stopped"))?;
                }
            }
        }
        if !batch.is_empty() {
            tx.blocking_send(batch)
                .map_err(|_| anyhow::anyhow!("writer stopped"))?;
        }
        Ok((bodies, main_stars))
    });
    let mut written = 0u64;
    let mut failure = None;
    while let Some(batch) = rx.recv().await {
        match apply_stars(pool, &batch).await {
            Ok(n) => written += n,
            Err(e) => {
                failure = Some(e);
                break;
            }
        }
    }
    drop(rx);
    let counts = reader_task.await.context("bodies reader panicked")?;
    let (bodies, main_stars) = match (failure, counts) {
        (Some(e), _) | (None, Err(e)) => {
            let _ = sqlx::query("UPDATE service_hydrations SET status = 'failed', completed_at = now(), rows_applied = $1 WHERE id = $2")
                .bind(i64::try_from(written)?)
                .bind(run_id)
                .execute(pool)
                .await;
            return Err(e.context("applying EDSM bodies"));
        }
        (None, Ok(c)) => c,
    };
    sqlx::query("UPDATE service_hydrations SET status = 'complete', completed_at = now(), rows_applied = $1 WHERE id = $2")
        .bind(i64::try_from(written)?)
        .bind(run_id)
        .execute(pool)
        .await?;
    tracing::info!(bodies, main_stars, written, "EDSM bodies hydrated");
    Ok(EdsmBodiesResult {
        source,
        bodies,
        main_stars,
        written,
    })
}

#[derive(Debug, Serialize)]
pub struct StarsPublication {
    pub version: String,
    pub stars: u64,
    pub artifact: ArtifactFile,
}

/// Publish `stars` as the `stars` product: one stars section, streamed
/// from the table in address order, into `stars/<version>/stars-<version>.ebex.zst`.
pub async fn publish_stars(pool: &PgPool, artifact_dir: &Path) -> Result<StarsPublication> {
    let (sequence, created_at, generated_at): (i64, i64, String) = sqlx::query_as(
        "INSERT INTO artifact_publications (product, status) VALUES ('stars', 'building') \
         RETURNING id, EXTRACT(EPOCH FROM created_at)::BIGINT, \
         to_char(created_at AT TIME ZONE 'UTC', 'YYYY-MM-DD\"T\"HH24:MI:SS\"Z\"')",
    )
    .fetch_one(pool)
    .await?;
    let result = build_stars(pool, artifact_dir, sequence, created_at, &generated_at).await;
    match &result {
        Ok(p) => {
            sqlx::query("UPDATE artifact_publications SET status = 'complete', artifact_path = $2, artifact_bytes = $3, artifact_sha256 = $4, completed_at = now() WHERE id = $1")
                .bind(sequence).bind(&p.artifact.path).bind(i64::try_from(p.artifact.bytes)?).bind(&p.artifact.sha256)
                .execute(pool).await?;
        }
        Err(_) => {
            let _ = sqlx::query("UPDATE artifact_publications SET status = 'failed', completed_at = now() WHERE id = $1").bind(sequence).execute(pool).await;
        }
    }
    result
}

async fn build_stars(
    pool: &PgPool,
    artifact_dir: &Path,
    sequence: i64,
    created_at: i64,
    generated_at: &str,
) -> Result<StarsPublication> {
    use futures_util::TryStreamExt;
    let version = crate::version::short_version("stars", sequence, generated_at);
    let filename = format!("stars-{version}.ebex.zst");
    let staging = artifact_dir.join(format!(".staging-stars-{version}"));
    tokio::fs::create_dir_all(&staging).await?;
    let raw = staging.join(format!("stars-{version}.ebex"));
    let compressed = staging.join(&filename);
    let watermark: i64 = sqlx::query_scalar(
        "SELECT COALESCE(EXTRACT(EPOCH FROM MAX(observed_at))::BIGINT, 0) FROM stars",
    )
    .fetch_one(pool)
    .await?;
    let plan = [ed_ebex::stars_section_plan()];
    let mut writer = SnapshotWriter::create(
        &raw,
        SnapshotHeader {
            sequence: u64::try_from(sequence)?,
            created_at,
            watermark,
        },
        &plan,
    )?;
    writer.begin_section(SECTION_STARS)?;
    let mut count = 0u64;
    {
        let mut buffer = Vec::with_capacity(STAR_RECORD_BYTES as usize);
        let mut rows = sqlx::query_as::<_, (i64, i16, bool, i64)>(
            "SELECT address, class, scoopable, EXTRACT(EPOCH FROM observed_at)::BIGINT FROM stars ORDER BY address",
        )
        .fetch(pool);
        while let Some((address, class, scoopable, observed_at)) = rows.try_next().await? {
            buffer.clear();
            StarRecord {
                address,
                class: u8::try_from(class)?,
                scoopable,
                observed_at,
            }
            .encode_into(&mut buffer);
            writer.write_record(&buffer)?;
            count += 1;
        }
    }
    writer.end_section()?;
    writer.finish()?;
    {
        // finish() above already ran the certifying container walk over
        // this exact file; the directory parse is all a lookup needs.
        let map = ed_ebex::map_file(&raw)?;
        let section = ed_ebex::sections_prevalidated(&map)?
            .into_iter()
            .find(|section| section.id == SECTION_STARS)
            .context("stars section missing")?;
        ed_ebex::validate_stars_section(section)?;
    }
    let (bytes, sha256) = ed_ebex::compress_file(&raw, &compressed, 9)?;
    tokio::fs::remove_file(&raw).await?;
    let relative = format!("stars/{version}/{filename}");
    let artifact = ArtifactFile {
        path: relative,
        bytes,
        sha256,
    };
    artifact.validate()?;
    let published = artifact_dir.join("stars").join(&version);
    tokio::fs::create_dir_all(artifact_dir.join("stars")).await?;
    tokio::fs::rename(&staging, &published)
        .await
        .context("atomically publishing stars artifact")?;
    let manifest = Manifest::with_product(
        crate::routing::read_current_manifest(artifact_dir)?,
        generated_at,
        None,
        ProductKey::Stars,
        Product {
            version: version.clone(),
            schema: ed_sync::STARS_SCHEMA_V1,
            minimum_client: None,
            files: vec![artifact.clone()],
            overlays: Vec::new(),
            covers_from: None,
        },
    );
    crate::routing::write_manifest(artifact_dir, &manifest, &format!("stars-{version}"))?;
    Ok(StarsPublication {
        version,
        stars: count,
        artifact,
    })
}

// ── the lookup endpoint and its queue ──

#[derive(Debug, Deserialize)]
pub struct StarsQuery {
    /// Comma-separated system addresses (id64), at most 500.
    pub ids: String,
}

#[derive(Debug, Serialize, PartialEq, Eq)]
pub struct KnownStar {
    pub id64: i64,
    pub class: u8,
    pub scoopable: bool,
    pub observed_at: i64,
}

#[derive(Debug, Serialize, Default, PartialEq, Eq)]
pub struct StarsAnswer {
    pub known: Vec<KnownStar>,
    /// Addresses the store cannot answer yet; queued for an EDSM lookup,
    /// so ask again later.
    pub queued: Vec<i64>,
}

pub const MAX_LOOKUP_IDS: usize = 500;

/// Answer what the store knows and queue the rest.
pub async fn answer_stars(pool: &PgPool, ids: &[i64]) -> Result<StarsAnswer> {
    anyhow::ensure!(
        ids.len() <= MAX_LOOKUP_IDS,
        "at most {MAX_LOOKUP_IDS} ids per request"
    );
    let ids: Vec<i64> = ids.iter().copied().filter(|id| *id > 0).collect();
    let known: Vec<(i64, i16, bool, i64)> = sqlx::query_as(
        "SELECT address, class, scoopable, EXTRACT(EPOCH FROM observed_at)::BIGINT FROM stars WHERE address = ANY($1) ORDER BY address",
    )
    .bind(&ids)
    .fetch_all(pool)
    .await?;
    let known_ids: std::collections::BTreeSet<i64> = known.iter().map(|k| k.0).collect();
    let queued: Vec<i64> = ids
        .iter()
        .copied()
        .filter(|id| !known_ids.contains(id))
        .collect();
    if !queued.is_empty() {
        sqlx::query("INSERT INTO star_lookups (address) SELECT unnest($1::bigint[]) ON CONFLICT (address) DO NOTHING")
            .bind(&queued)
            .execute(pool)
            .await?;
    }
    metrics::counter!("edda_stars_requests_total").increment(1);
    metrics::counter!("edda_stars_ids_total", "answer" => "known").increment(known.len() as u64);
    metrics::counter!("edda_stars_ids_total", "answer" => "queued").increment(queued.len() as u64);
    Ok(StarsAnswer {
        known: known
            .into_iter()
            .map(|(id64, class, scoopable, observed_at)| KnownStar {
                id64,
                class: class as u8,
                scoopable,
                observed_at,
            })
            .collect(),
        queued,
    })
}

/// Drain the lookup queue against EDSM, one system per second, forever.
/// Systems EDSM does not know are dropped after three attempts.
pub async fn run_edsm_lookups(pool: PgPool, client: reqwest::Client) {
    loop {
        let next: Option<(i64, i32)> = sqlx::query_as(
            "SELECT address, attempts FROM star_lookups WHERE attempts < 3 ORDER BY requested_at LIMIT 1",
        )
        .fetch_optional(&pool)
        .await
        .unwrap_or(None);
        let Some((address, _)) = next else {
            tokio::time::sleep(std::time::Duration::from_secs(5)).await;
            continue;
        };
        match edsm_primary_star(&client, address).await {
            Ok(Some(star)) => {
                metrics::counter!("edda_edsm_lookups_total", "outcome" => "resolved").increment(1);
                if let Err(e) = apply_stars(&pool, &[star]).await {
                    tracing::warn!(error = %e, address, "EDSM lookup could not be stored");
                }
            }
            Ok(None) => {
                metrics::counter!("edda_edsm_lookups_total", "outcome" => "unknown").increment(1);
                let _ = sqlx::query("UPDATE star_lookups SET attempts = 3, last_error = 'EDSM has no primary star' WHERE address = $1")
                    .bind(address).execute(&pool).await;
            }
            Err(e) => {
                metrics::counter!("edda_edsm_lookups_total", "outcome" => "error").increment(1);
                let _ = sqlx::query("UPDATE star_lookups SET attempts = attempts + 1, last_error = $2 WHERE address = $1")
                    .bind(address).bind(e.to_string()).execute(&pool).await;
                tracing::warn!(error = %e, address, "EDSM lookup failed");
            }
        }
        tokio::time::sleep(std::time::Duration::from_secs(1)).await;
    }
}

/// One EDSM `api-v1/system` call by id64 for the primary star.
async fn edsm_primary_star(
    client: &reqwest::Client,
    address: i64,
) -> Result<Option<StarObservation>> {
    let url = format!(
        "https://www.edsm.net/api-v1/system?systemId64={address}&showPrimaryStar=1&showId=1"
    );
    let v: serde_json::Value = client
        .get(&url)
        .header(
            reqwest::header::USER_AGENT,
            "EDDA-API/0.1 (edda community server)",
        )
        .timeout(std::time::Duration::from_secs(30))
        .send()
        .await?
        .error_for_status()?
        .json()
        .await?;
    Ok(edsm_system_primary_star(&v, address))
}

/// The primary star of an EDSM `api-v1/system` answer, if it carries one.
pub fn edsm_system_primary_star(v: &serde_json::Value, address: i64) -> Option<StarObservation> {
    let star = v.get("primaryStar")?;
    let subtype = star.get("type")?.as_str()?.to_string();
    let class = StarClass::from_subtype(&subtype);
    if class == StarClass::Unknown {
        return None;
    }
    Some(StarObservation {
        address,
        scoopable: star
            .get("isScoopable")
            .and_then(|b| b.as_bool())
            .unwrap_or_else(|| class.scoopable()),
        subtype,
        class,
        observed_at: std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_secs() as i64)
            .unwrap_or(0),
        source: "edsm api".into(),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Stage 1c's seam: a Spansh galaxy-dump body teaches its system's
    /// main-star class — and only a main star, only a Star, only a class
    /// the index has a code for.
    #[test]
    fn spansh_main_star_bodies_become_observations() {
        let body = |kind: &str, sub: Option<&str>, main| ed_store::galaxy::spansh::Body {
            kind: Some(kind.to_string()),
            sub_type: sub.map(String::from),
            main_star: main,
            ..Default::default()
        };
        let star = spansh_body_main_star(
            42,
            &body("Star", Some("Neutron Star"), true),
            Some(1_756_900_000),
            "spansh:galaxy:galaxy_7days.json.gz",
        )
        .unwrap();
        assert_eq!(
            (star.address, star.class, star.scoopable, star.observed_at),
            (42, StarClass::Neutron, false, 1_756_900_000)
        );
        let scoopy = spansh_body_main_star(
            7,
            &body("Star", Some("K (Yellow-Orange) Star"), true),
            None,
            "s",
        )
        .unwrap();
        assert!(scoopy.scoopable && scoopy.observed_at == 0);
        assert!(
            spansh_body_main_star(42, &body("Star", Some("Neutron Star"), false), None, "s")
                .is_none(),
            "secondary star"
        );
        assert!(
            spansh_body_main_star(42, &body("Planet", Some("Icy body"), true), None, "s").is_none(),
            "not a star"
        );
        assert!(
            spansh_body_main_star(42, &body("Star", None, true), None, "s").is_none(),
            "no subtype"
        );
        assert!(
            spansh_body_main_star(0, &body("Star", Some("Neutron Star"), true), None, "s")
                .is_none(),
            "bad address"
        );
    }

    /// The bug the first galaxy_7days run found: a batch with the same
    /// address twice kills the upsert ("ON CONFLICT DO UPDATE command
    /// cannot affect row a second time"). Dedupe keeps the newest, and
    /// the first listed on ties (a two-main-star system's primary).
    #[test]
    fn duplicate_addresses_collapse_to_the_newest_observation() {
        let star = |address: i64, subtype: &str, observed_at: i64| StarObservation {
            address,
            subtype: subtype.into(),
            class: StarClass::from_subtype(subtype),
            scoopable: false,
            observed_at,
            source: "test".into(),
        };
        let out = dedupe_newest(&[
            star(1, "Neutron Star", 100),
            star(2, "K (Yellow-Orange) Star", 50),
            star(1, "White Dwarf (DA) Star", 200), // newer wins
            star(2, "M (Red dwarf) Star", 50),     // tie: first listed wins
        ]);
        assert_eq!(out.len(), 2);
        assert_eq!(out[0].observed_at, 200, "address 1 kept the newer");
        assert_eq!(
            out[1].subtype, "K (Yellow-Orange) Star",
            "address 2 kept the first on a tie"
        );
    }

    #[test]
    fn edsm_body_lines_yield_main_stars_only() {
        let star = r#"{"id":1,"id64":19594034844259,"bodyId":0,"name":"Schee Flyuae ZF-F d11-570","type":"Star","subType":"White Dwarf (DC) Star","isMainStar":true,"isScoopable":false,"updateTime":"2026-08-23 06:53:55","systemId64":19594034844259,"systemName":"Schee Flyuae ZF-F d11-570"},"#;
        let s = edsm_body_main_star(star, "edsm:bodies:test").unwrap();
        assert_eq!(s.address, 19594034844259);
        assert_eq!(s.class, StarClass::WhiteDwarf);
        assert!(!s.scoopable);
        assert_eq!(
            s.observed_at,
            parse_edsm_time("2026-08-23 06:53:55").unwrap()
        );
        let planet =
            r#"{"type":"Planet","subType":"Rocky body","isMainStar":false,"systemId64":5}"#;
        assert!(edsm_body_main_star(planet, "x").is_none());
        let secondary = r#"{"type":"Star","subType":"K (Yellow-Orange) Star","isMainStar":false,"systemId64":5}"#;
        assert!(edsm_body_main_star(secondary, "x").is_none());
        assert!(edsm_body_main_star("[", "x").is_none());
        assert!(edsm_body_main_star("{not json", "x").is_none());
    }

    #[test]
    fn edsm_times_are_utc_epochs() {
        assert_eq!(parse_edsm_time("1970-01-01 00:00:00"), Some(0));
        assert_eq!(parse_edsm_time("2026-08-23 06:53:55"), Some(1_787_468_035));
        assert_eq!(parse_edsm_time("nope"), None);
    }

    #[test]
    fn edsm_system_answers_yield_the_primary_star() {
        let v: serde_json::Value = serde_json::json!({"name":"Sol","id64":10477373803_i64,"primaryStar":{"type":"G (White-Yellow) Star","name":"Sol","isScoopable":true}});
        let s = edsm_system_primary_star(&v, 10477373803).unwrap();
        assert_eq!(s.class, StarClass::G);
        assert!(s.scoopable);
        assert!(edsm_system_primary_star(&serde_json::json!({}), 1).is_none());
        assert!(edsm_system_primary_star(
            &serde_json::json!({"primaryStar":{"type":"?? not a star type"}}),
            1
        )
        .is_none());
    }
}
