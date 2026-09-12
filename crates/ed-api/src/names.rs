//! Name completion — the server half of the client's `name_complete`.
//!
//! RULING (maintainer, 2026-09-07): the server knows more than every client.
//! A remote-first install carries only the bundled bubble index (145,579
//! names), so anywhere the app expects a system or station name typed,
//! the suggestions have to come from here too. Systems complete against
//! the mapped routing index — the same `Galaxy::complete` the client runs
//! locally, so both halves agree — and stations against Postgres, where
//! the identity table is the fleet's only complete list.
//!
//! Measured on the day it lands: `edda_names_complete_requests_total
//! {kind,outcome}` and `edda_names_complete_seconds{kind}` (the handler
//! records both); the station query has its own prefix index (migration
//! 0014) so it is an index range scan, not a seq scan over 830k rows.

use anyhow::Result;
use sqlx::PgPool;

/// One suggestion, in the client's `NameHit` shape.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct NameHit {
    pub name: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub detail: Option<String>,
}

/// Shortest prefix worth answering: one character of a galaxy-wide
/// index is every system starting with that letter.
pub const MIN_PREFIX: usize = 2;
pub const MAX_PREFIX: usize = 64;
pub const DEFAULT_LIMIT: usize = 12;
pub const MAX_LIMIT: usize = 25;

/// Normalise a raw prefix: trimmed, length-checked. `None` means "not a
/// question" (too short or too long), answered as an empty list.
pub fn usable_prefix(raw: &str) -> Option<&str> {
    let p = raw.trim();
    (p.chars().count() >= MIN_PREFIX && p.chars().count() <= MAX_PREFIX).then_some(p)
}

pub fn clamp_limit(limit: Option<usize>) -> usize {
    limit.unwrap_or(DEFAULT_LIMIT).clamp(1, MAX_LIMIT)
}

/// Systems starting with `prefix`, case-insensitive, from the routing
/// index. Same call the client makes on its own copy of the index.
pub fn complete_systems(galaxy: &ed_galaxy::Galaxy, prefix: &str, limit: usize) -> Vec<NameHit> {
    galaxy
        .complete(prefix, limit)
        .into_iter()
        .map(|idx| {
            let r = galaxy.record(idx);
            NameHit {
                name: galaxy.name(&r).to_string(),
                detail: None,
            }
        })
        .collect()
}

/// Stations starting with `prefix`, with the system as the detail so two
/// "Jameson Memorial"s are distinguishable. Ordered by name for stable
/// suggestions.
pub async fn complete_stations(pool: &PgPool, prefix: &str, limit: usize) -> Result<Vec<NameHit>> {
    // `lower(name) LIKE lower($1) || '%'` is what the text_pattern_ops
    // index in migration 0014 serves; `%` and `_` in the prefix would be
    // wildcards, so they are escaped to stay literal.
    let pattern = format!(
        "{}%",
        prefix
            .replace('\\', "\\\\")
            .replace('%', "\\%")
            .replace('_', "\\_")
    );
    let rows: Vec<(String, Option<String>)> = sqlx::query_as(
        "SELECT st.name, s.name \
         FROM stations st LEFT JOIN systems s ON s.address = st.system_address \
         WHERE lower(st.name) LIKE lower($1) \
         ORDER BY st.name \
         LIMIT $2",
    )
    .bind(&pattern)
    .bind(limit as i64)
    .fetch_all(pool)
    .await?;
    Ok(rows
        .into_iter()
        .map(|(name, system)| NameHit {
            name,
            detail: system,
        })
        .collect())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tiny_galaxy() -> (tempfile::TempDir, ed_galaxy::Galaxy) {
        let dir = tempfile::tempdir().unwrap();
        let source = r#"[
{"id64":10477373803,"name":"Sol","coords":{"x":0,"y":0,"z":0},"bodies":[{"type":"Star","subType":"G (White-Yellow) Star","mainStar":true}]},
{"id64":2,"name":"Solati","coords":{"x":10,"y":0,"z":0},"bodies":[{"type":"Star","subType":"K (Yellow-Orange) Star","mainStar":true}]},
{"id64":3,"name":"Sothis","coords":{"x":20,"y":0,"z":0},"bodies":[{"type":"Star","subType":"M (Red dwarf) Star","mainStar":true}]},
{"id64":4,"name":"Deciat","coords":{"x":30,"y":0,"z":0},"bodies":[{"type":"Star","subType":"K (Yellow-Orange) Star","mainStar":true}]}
]"#;
        let path = dir.path().join("galaxy");
        ed_galaxy::import::import_reader(Box::new(source.as_bytes()), &path, &mut |_| {}).unwrap();
        let galaxy = ed_galaxy::Galaxy::open(&path).unwrap();
        (dir, galaxy)
    }

    #[test]
    fn systems_complete_case_insensitively_and_in_name_order() {
        let (_dir, g) = tiny_galaxy();
        let names: Vec<String> = complete_systems(&g, "so", 10)
            .into_iter()
            .map(|h| h.name)
            .collect();
        assert_eq!(names, vec!["Sol", "Solati", "Sothis"]);
        let names: Vec<String> = complete_systems(&g, "SOL", 10)
            .into_iter()
            .map(|h| h.name)
            .collect();
        assert_eq!(names, vec!["Sol", "Solati"]);
        assert!(complete_systems(&g, "zz", 10).is_empty());
    }

    #[test]
    fn the_limit_is_honoured() {
        let (_dir, g) = tiny_galaxy();
        assert_eq!(complete_systems(&g, "so", 2).len(), 2);
    }

    #[test]
    fn prefix_and_limit_rules() {
        assert_eq!(usable_prefix("  so "), Some("so"));
        assert_eq!(
            usable_prefix("s"),
            None,
            "one letter is every system starting with it"
        );
        assert_eq!(usable_prefix(&"x".repeat(65)), None);
        assert_eq!(clamp_limit(None), DEFAULT_LIMIT);
        assert_eq!(clamp_limit(Some(0)), 1);
        assert_eq!(clamp_limit(Some(500)), MAX_LIMIT);
    }

    #[test]
    fn hits_serialise_in_the_clients_shape() {
        let hit = NameHit {
            name: "Jameson Memorial".into(),
            detail: Some("Shinrarta Dezhra".into()),
        };
        assert_eq!(
            serde_json::to_string(&hit).unwrap(),
            r#"{"name":"Jameson Memorial","detail":"Shinrarta Dezhra"}"#
        );
        let bare = NameHit {
            name: "Sol".into(),
            detail: None,
        };
        assert_eq!(serde_json::to_string(&bare).unwrap(), r#"{"name":"Sol"}"#);
    }
}
