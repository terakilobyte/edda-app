//! Public product versions: short hashes, not the shared numeric
//! publication sequence.
//!
//! Maintainer ruling (2026-09-04): "instead of numeric where it's confusing
//! for a viewer, just use short hashes" — routing 52 / stars 51 /
//! community 54 read like relations that do not exist, because every
//! product draws from one `artifact_publications` sequence. The public
//! version becomes 8 hex of sha256 over product|sequence|created_at; the
//! numeric row id stays internal (it still keys the publication rows and
//! the manifest backups). The sequence in the preimage makes cross-
//! product equality impossible in practice, and 8 hex keeps the odds of
//! any two versions of one product colliding below 1e-8 per pair.

use sha2::Digest as _;

/// The public version identifier for a publication.
pub fn short_version(product: &str, sequence: i64, generated_at: &str) -> String {
    let digest = sha2::Sha256::digest(format!("{product}|{sequence}|{generated_at}").as_bytes());
    digest[..4]
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect()
}

/// Is `name` shaped like a published version directory: the legacy
/// numeric sequence, or an 8-hex short hash. Anything else (sub-index
/// dirs, a human's scratch) is not a publication and must never prune.
pub fn is_version_dir_name(name: &str) -> bool {
    name.parse::<u64>().is_ok()
        || (name.len() == 8 && name.bytes().all(|b| matches!(b, b'0'..=b'9' | b'a'..=b'f')))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn short_versions_are_stable_distinct_and_dir_shaped() {
        let a = short_version("routing", 52, "2026-09-04T05:00:00Z");
        assert_eq!(
            a,
            short_version("routing", 52, "2026-09-04T05:00:00Z"),
            "deterministic"
        );
        assert_ne!(
            a,
            short_version("stars", 52, "2026-09-04T05:00:00Z"),
            "products diverge at the same sequence"
        );
        assert_ne!(a, short_version("routing", 53, "2026-09-04T05:00:00Z"));
        assert!(is_version_dir_name(&a));
        assert_eq!(a.len(), 8);
    }

    #[test]
    fn version_dir_shapes_cover_legacy_and_hash_but_not_scratch() {
        assert!(is_version_dir_name("51"), "legacy numeric");
        assert!(is_version_dir_name("00c0ffee"), "short hash");
        assert!(
            is_version_dir_name("12345678"),
            "numeric-looking hash is both, still a version"
        );
        assert!(!is_version_dir_name("overlays"));
        assert!(!is_version_dir_name("DEADBEEF"), "uppercase is not ours");
        assert!(!is_version_dir_name("abc"), "wrong length, not numeric");
        assert!(!is_version_dir_name("boost250"));
    }
}
