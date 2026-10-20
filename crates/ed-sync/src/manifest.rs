//! Versioned publication manifest shared by `ed-api` (producer) and the
//! desktop client (consumer).
//!
//! The JSON shape is protocol 1 from `docs/API-SERVICE-DESIGN.md`:
//!
//! ```json
//! {
//!   "protocol": 1,
//!   "generated_at": "2026-08-30T07:00:00Z",
//!   "eddn_watermark": "2026-08-30T06:59:58Z",
//!   "products": {
//!     "stars": {
//!       "version": "997953b4",
//!       "schema": 1,
//!       "files": [{"path": "stars/997953b4/stars-997953b4.ebex.zst", "bytes": 14575452, "sha256": "..."}]
//!     }
//!   }
//! }
//! ```
//!
//! Product keys are a closed enum so the producer and the consumer cannot
//! disagree on the string a product is published under. Unknown product keys
//! in a received manifest are ignored, as the compatibility policy requires;
//! that is also how the retired keys (`bootstrap`, `community`,
//! `market_daily`, the market baselines a hydrating client installed until
//! 0.3.0; retired 2026-10-20) read when an old manifest still names them.

use std::{collections::BTreeMap, fmt, io::Read, path::Path, str::FromStr};

use anyhow::{ensure, Context, Result};
use serde::{Deserialize, Deserializer, Serialize};

use crate::digest::{is_sha256_hex, sha256_hex, verify_sha256_reader};

pub const MANIFEST_PROTOCOL_V1: u32 = 1;
/// The `stars` product's schema: the stars section schema.
pub const STARS_SCHEMA_V1: u32 = ed_ebex::STAR_SCHEMA_V1 as u32;
pub const API_V1_PREFIX: &str = "/v1";
pub const MANIFEST_ROUTE: &str = "/v1/manifest";
pub const ARTIFACT_ROUTE_PREFIX: &str = "/v1/artifacts/";

/// Every product the protocol knows how to publish. The serialized form is
/// the lowercase variant name.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum ProductKey {
    /// Versioned `ed-galaxy` routing index.
    Routing,
    /// Main-star classes learned beyond the routing base (one stars
    /// section); small, published weekly.
    Stars,
}

impl ProductKey {
    pub const ALL: [ProductKey; 2] = [ProductKey::Routing, ProductKey::Stars];

    pub const fn as_str(self) -> &'static str {
        match self {
            ProductKey::Routing => "routing",
            ProductKey::Stars => "stars",
        }
    }
}

impl fmt::Display for ProductKey {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

impl FromStr for ProductKey {
    type Err = anyhow::Error;

    fn from_str(value: &str) -> Result<Self> {
        ProductKey::ALL
            .into_iter()
            .find(|key| key.as_str() == value)
            .with_context(|| format!("unknown manifest product {value:?}"))
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Deserialize, Serialize)]
pub struct Manifest {
    pub protocol: u32,
    pub generated_at: String,
    #[serde(default)]
    pub eddn_watermark: Option<String>,
    #[serde(deserialize_with = "known_products")]
    pub products: BTreeMap<ProductKey, Product>,
}

#[derive(Clone, Debug, PartialEq, Eq, Deserialize, Serialize)]
pub struct Product {
    pub version: String,
    pub schema: u32,
    #[serde(default)]
    pub minimum_client: Option<String>,
    pub files: Vec<ArtifactFile>,
    /// Item 47: the overlay chain. Each link carries a client holding
    /// `from` to `to` with a small delta artifact instead of the full
    /// product. Links are orderable by following `from` -> `to`; a client
    /// whose installed version has no path to `version` falls back to the
    /// full artifact download. Absent on non-routing products and in
    /// manifests from older servers (defaults to empty; older clients
    /// ignore the unknown field).
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub overlays: Vec<OverlayLink>,
}

/// One link of a product's overlay chain: the delta artifact that carries
/// version `from` to version `to`.
#[derive(Clone, Debug, PartialEq, Eq, Deserialize, Serialize)]
pub struct OverlayLink {
    pub from: String,
    pub to: String,
    #[serde(flatten)]
    pub artifact: ArtifactFile,
}

#[derive(Clone, Debug, PartialEq, Eq, Deserialize, Serialize)]
pub struct ArtifactFile {
    /// Relative, `/`-separated path below the artifact root; also the tail
    /// of the download URL.
    pub path: String,
    pub bytes: u64,
    /// Lowercase hex SHA-256 of the file as served.
    pub sha256: String,
}

/// Unknown product keys are skipped rather than failing the whole manifest.
fn known_products<'de, D: Deserializer<'de>>(
    deserializer: D,
) -> std::result::Result<BTreeMap<ProductKey, Product>, D::Error> {
    let raw = BTreeMap::<String, Product>::deserialize(deserializer)?;
    let mut products = BTreeMap::new();
    for (key, product) in raw {
        if let Ok(key) = key.parse::<ProductKey>() {
            products.insert(key, product);
        }
    }
    // An empty map is a validation failure, not a deserialization failure.
    Ok(products)
}

impl Manifest {
    /// The manifest after publishing `product` under `key`: every other
    /// product of `existing` is carried forward unchanged, so publishing
    /// one product never unpublishes another. `eddn_watermark` replaces
    /// the existing one only when the publication supplies it.
    pub fn with_product(
        existing: Option<Manifest>,
        generated_at: impl Into<String>,
        eddn_watermark: Option<String>,
        key: ProductKey,
        product: Product,
    ) -> Self {
        let mut manifest = existing.unwrap_or(Manifest {
            protocol: MANIFEST_PROTOCOL_V1,
            generated_at: String::new(),
            eddn_watermark: None,
            products: BTreeMap::new(),
        });
        manifest.protocol = MANIFEST_PROTOCOL_V1;
        manifest.generated_at = generated_at.into();
        if eddn_watermark.is_some() {
            manifest.eddn_watermark = eddn_watermark;
        }
        manifest.products.insert(key, product);
        manifest
    }

    pub fn validate(&self) -> Result<()> {
        ensure!(
            self.protocol == MANIFEST_PROTOCOL_V1,
            "unsupported manifest protocol {}",
            self.protocol
        );
        ensure!(
            !self.generated_at.trim().is_empty(),
            "manifest generated_at is empty"
        );
        ensure!(!self.products.is_empty(), "manifest contains no products");
        for (key, product) in &self.products {
            product
                .validate()
                .with_context(|| format!("manifest product {key} is invalid"))?;
        }
        Ok(())
    }

    /// Every artifact across every product — full files and overlay
    /// deltas alike — for serving by path.
    pub fn artifact(&self, path: &str) -> Option<&ArtifactFile> {
        self.products
            .values()
            .flat_map(|product| {
                product
                    .files
                    .iter()
                    .chain(product.overlays.iter().map(|link| &link.artifact))
            })
            .find(|file| file.path == path)
    }
}

impl Product {
    pub fn validate(&self) -> Result<()> {
        ensure!(!self.version.trim().is_empty(), "product version is empty");
        ensure!(self.schema != 0, "product schema is zero");
        ensure!(!self.files.is_empty(), "product contains no artifacts");
        for file in &self.files {
            file.validate()?;
        }
        let mut froms = std::collections::BTreeSet::new();
        for link in &self.overlays {
            link.validate()?;
            ensure!(
                froms.insert(&link.from),
                "overlay chain has two links from version {:?}",
                link.from
            );
        }
        Ok(())
    }

    /// The overlay links that carry `installed` to this product's version,
    /// in application order. `Some(vec![])` means already current; `None`
    /// means no overlay path exists and the client must take the full
    /// artifact download instead.
    pub fn overlay_chain(&self, installed: &str) -> Option<Vec<&OverlayLink>> {
        let mut chain = Vec::new();
        let mut at = installed;
        while at != self.version {
            let link = self.overlays.iter().find(|link| link.from == at)?;
            // A cycle can never reach `version`; the duplicate-`from` check
            // in validate() makes loops unrepresentable after validation,
            // but stay finite on unvalidated input too.
            if chain.len() >= self.overlays.len() {
                return None;
            }
            at = &link.to;
            chain.push(link);
        }
        Some(chain)
    }
}

impl OverlayLink {
    pub fn validate(&self) -> Result<()> {
        ensure!(!self.from.trim().is_empty(), "overlay `from` is empty");
        ensure!(!self.to.trim().is_empty(), "overlay `to` is empty");
        ensure!(
            self.from != self.to,
            "overlay from and to are both {:?}",
            self.from
        );
        self.artifact.validate()
    }
}

impl ArtifactFile {
    /// Describe `bytes` as they will be served under `path`.
    pub fn for_bytes(path: impl Into<String>, bytes: &[u8]) -> Self {
        ArtifactFile {
            path: path.into(),
            bytes: bytes.len() as u64,
            sha256: sha256_hex(bytes),
        }
    }

    pub fn validate(&self) -> Result<()> {
        ensure!(self.bytes > 0, "artifact {} is empty", self.path);
        ensure!(
            is_sha256_hex(&self.sha256),
            "artifact {} has an invalid sha256",
            self.path
        );
        ensure!(
            is_safe_relative_path(&self.path),
            "artifact path {:?} is unsafe",
            self.path
        );
        Ok(())
    }

    /// URL path (below the API origin) this artifact is downloaded from.
    pub fn route(&self) -> String {
        format!("{ARTIFACT_ROUTE_PREFIX}{}", self.path)
    }

    /// Verify a fully downloaded copy of this artifact: exact length, then
    /// the streaming SHA-256.
    pub fn verify_reader(&self, reader: impl Read) -> Result<()> {
        verify_sha256_reader(reader, &self.sha256)
            .with_context(|| format!("artifact {} failed verification", self.path))
    }

    pub fn verify_file(&self, path: &Path) -> Result<()> {
        let file =
            std::fs::File::open(path).with_context(|| format!("opening {}", path.display()))?;
        let length = file.metadata()?.len();
        ensure!(
            length == self.bytes,
            "artifact {} is {length} bytes; manifest says {}",
            self.path,
            self.bytes
        );
        self.verify_reader(file)
    }
}

/// A relative `/`-separated path with no empty, `.`, `..`, or
/// backslash-containing components.
pub fn is_safe_relative_path(path: &str) -> bool {
    !path.is_empty()
        && !path.starts_with('/')
        && path
            .split('/')
            .all(|part| !part.is_empty() && part != "." && part != ".." && !part.contains('\\'))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn artifact() -> ArtifactFile {
        ArtifactFile {
            path: "stars/1/stars-1.ebex.zst".into(),
            bytes: 123,
            sha256: "0".repeat(64),
        }
    }

    fn valid_manifest() -> Manifest {
        Manifest::with_product(
            None,
            "2026-08-29T12:00:00Z",
            Some("2026-08-29T11:59:00Z".into()),
            ProductKey::Stars,
            Product {
                version: "1".into(),
                schema: STARS_SCHEMA_V1,
                minimum_client: None,
                files: vec![artifact()],
                overlays: Vec::new(),
            },
        )
    }

    #[test]
    fn product_key_serde_form_matches_from_str_for_every_variant() {
        for key in ProductKey::ALL {
            let json = serde_json::to_string(&key).unwrap();
            assert_eq!(json, format!("\"{}\"", key.as_str()));
            assert_eq!(serde_json::from_str::<ProductKey>(&json).unwrap(), key);
            assert_eq!(key.as_str().parse::<ProductKey>().unwrap(), key);
        }
        assert!("market".parse::<ProductKey>().is_err());
        for retired in ["bootstrap", "community", "market_daily"] {
            assert!(retired.parse::<ProductKey>().is_err(), "{retired} was retired 2026-10-20");
        }
    }

    /// The wire contract of a publication: the serialized key is the
    /// string the server publishes under and the reader parses back.
    #[test]
    fn server_publication_round_trips_through_the_reader() {
        let published = serde_json::to_vec_pretty(&valid_manifest()).unwrap();
        let text = std::str::from_utf8(&published).unwrap();
        assert!(text.contains("\"stars\""), "{text}");

        let received: Manifest = serde_json::from_slice(&published).unwrap();
        received.validate().unwrap();
        let product = &received.products[&ProductKey::Stars];
        assert_eq!(product.version, "1");
        assert_eq!(product.files[0], artifact());
        assert_eq!(product.files[0].route(), "/v1/artifacts/stars/1/stars-1.ebex.zst");
    }

    /// Retired keys and keys from the future read the same way: skipped.
    /// The box's `current.json` named `community` and `market_daily` until
    /// the 2026-10-20 one-off removed them; a reader built after the
    /// retirement must serve and parse it without them.
    #[test]
    fn retired_and_unknown_products_are_skipped() {
        let sketch = r#"{
          "protocol": 1,
          "generated_at": "2026-08-30T07:00:00Z",
          "eddn_watermark": "2026-08-30T06:59:58Z",
          "products": {
            "bootstrap": {
              "version": "2026-08-30.1",
              "schema": 1,
              "files": [{"path": "community-2026-08-30.1.ebex.zst", "bytes": 5000000, "sha256": "0000000000000000000000000000000000000000000000000000000000000000"}]
            },
            "community": {
              "version": "4703de92",
              "schema": 1,
              "files": [{"path": "community/4703de92/community-4703de92.ebex.zst", "bytes": 692785616, "sha256": "0000000000000000000000000000000000000000000000000000000000000000"}]
            },
            "market_daily": {
              "version": "9435b656",
              "schema": 1,
              "covers_from": 1760000000,
              "files": [{"path": "market_daily/9435b656/market-daily-9435b656.ebex.zst", "bytes": 59396789, "sha256": "0000000000000000000000000000000000000000000000000000000000000000"}]
            },
            "routing": {
              "version": "2026-08-30.1",
              "schema": 2,
              "files": [{"path": "stars.bin", "bytes": 1, "sha256": "0000000000000000000000000000000000000000000000000000000000000000"}]
            },
            "future-product": {"version": "x", "schema": 9, "files": []}
          }
        }"#;
        let manifest: Manifest = serde_json::from_str(sketch).unwrap();
        manifest.validate().unwrap();
        assert_eq!(
            manifest.products.keys().copied().collect::<Vec<_>>(),
            [ProductKey::Routing],
            "retired and unknown products are ignored"
        );
        assert!(manifest.artifact("community/4703de92/community-4703de92.ebex.zst").is_none());
    }

    #[test]
    fn rejects_unknown_protocols_and_empty_publications() {
        let mut manifest = valid_manifest();
        manifest.protocol = 2;
        assert!(manifest.validate().is_err());
        let mut manifest = valid_manifest();
        manifest.products.clear();
        assert!(manifest.validate().is_err());
    }

    #[test]
    fn rejects_product_without_artifacts() {
        let mut manifest = valid_manifest();
        manifest
            .products
            .get_mut(&ProductKey::Stars)
            .unwrap()
            .files
            .clear();
        assert!(manifest.validate().is_err());
    }

    #[test]
    fn rejects_invalid_artifact_metadata_and_unsafe_paths() {
        for (path, bytes, sha256) in [
            ("ok/file", 0u64, "0".repeat(64)),
            ("ok/file", 1, "not-a-hash".to_owned()),
            ("ok/file", 1, "0".repeat(63)),
            ("../secret", 1, "0".repeat(64)),
            ("stars\\..\\secret", 1, "0".repeat(64)),
            ("/etc/passwd", 1, "0".repeat(64)),
            ("a//b", 1, "0".repeat(64)),
            ("./a", 1, "0".repeat(64)),
            ("", 1, "0".repeat(64)),
        ] {
            let file = ArtifactFile {
                path: path.into(),
                bytes,
                sha256,
            };
            assert!(file.validate().is_err(), "accepted {file:?}");
        }
        assert!(is_safe_relative_path("stars/1/stars-1.ebex.zst"));
    }

    #[test]
    fn artifact_verification_checks_length_then_hash() {
        let payload = b"hello ebex";
        let artifact = ArtifactFile::for_bytes("a/b", payload);
        artifact.verify_reader(&payload[..]).unwrap();
        assert!(artifact.verify_reader(&b"hello ebe"[..]).is_err());

        let dir = std::env::temp_dir().join(format!("ed-sync-verify-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("artifact");
        std::fs::write(&path, payload).unwrap();
        artifact.verify_file(&path).unwrap();
        std::fs::write(&path, b"hello ebeX").unwrap();
        let error = artifact.verify_file(&path).unwrap_err().to_string();
        assert!(error.contains("failed verification"), "{error}");
        std::fs::write(&path, b"short").unwrap();
        let error = artifact.verify_file(&path).unwrap_err().to_string();
        assert!(error.contains("5 bytes"), "{error}");
        let _ = std::fs::remove_dir_all(dir);
    }

    fn overlay(from: &str, to: &str) -> OverlayLink {
        OverlayLink {
            from: from.into(),
            to: to.into(),
            artifact: ArtifactFile {
                path: format!("routing/overlays/{to}.edgo"),
                bytes: 42,
                sha256: "0".repeat(64),
            },
        }
    }

    fn routing_with_overlays() -> Product {
        Product {
            version: "47".into(),
            schema: 3,
            minimum_client: None,
            files: vec![artifact()],
            overlays: vec![overlay("45", "46"), overlay("46", "47")],
        }
    }

    /// The wire contract: overlays serialize flat ({from,to,path,bytes,
    /// sha256}), an old manifest without the field deserializes to an
    /// empty chain, and the round trip is lossless.
    #[test]
    fn overlay_chain_round_trips_and_defaults_empty() {
        let product = routing_with_overlays();
        product.validate().unwrap();
        let json = serde_json::to_string(&product).unwrap();
        assert!(json.contains("\"from\":\"45\""), "{json}");
        assert!(json.contains("\"path\":\"routing/overlays/46.edgo\""), "{json}");
        assert_eq!(serde_json::from_str::<Product>(&json).unwrap(), product);

        let old = r#"{"version":"45","schema":3,"files":[{"path":"routing/45/stars.bin","bytes":1,"sha256":"0000000000000000000000000000000000000000000000000000000000000000"}]}"#;
        let product: Product = serde_json::from_str(old).unwrap();
        assert!(product.overlays.is_empty());
        assert!(!serde_json::to_string(&product).unwrap().contains("overlays"));
    }

    #[test]
    fn overlay_chain_walks_from_installed_to_head() {
        let product = routing_with_overlays();
        let chain = product.overlay_chain("45").unwrap();
        assert_eq!(
            chain.iter().map(|l| (l.from.as_str(), l.to.as_str())).collect::<Vec<_>>(),
            [("45", "46"), ("46", "47")]
        );
        assert_eq!(product.overlay_chain("46").unwrap().len(), 1);
        assert_eq!(product.overlay_chain("47").unwrap().len(), 0, "already current");
        assert!(product.overlay_chain("8").is_none(), "no path: full download");
        assert!(product.overlay_chain("44").is_none(), "pruned base: full download");
    }

    #[test]
    fn overlay_chain_survives_a_cycle_without_hanging() {
        let mut product = routing_with_overlays();
        product.overlays = vec![overlay("45", "46"), overlay("46", "45")];
        assert!(product.overlay_chain("45").is_none());
        assert!(product.validate().is_ok(), "a cycle is two distinct froms");
    }

    #[test]
    fn product_validation_rejects_bad_overlays() {
        let mut product = routing_with_overlays();
        product.overlays.push(overlay("45", "47"));
        assert!(product.validate().is_err(), "duplicate from");

        let mut product = routing_with_overlays();
        product.overlays[0].to = "45".into();
        assert!(product.validate().is_err(), "from == to");

        let mut product = routing_with_overlays();
        product.overlays[0].artifact.bytes = 0;
        assert!(product.validate().is_err(), "empty artifact");

        let mut product = routing_with_overlays();
        product.overlays[0].artifact.path = "../escape".into();
        assert!(product.validate().is_err(), "unsafe path");
    }

    #[test]
    fn manifest_serves_artifacts_by_path() {
        let mut manifest = valid_manifest();
        assert!(manifest.artifact("stars/1/stars-1.ebex.zst").is_some());
        assert!(manifest.artifact("stars/1/other").is_none());

        manifest
            .products
            .insert(ProductKey::Routing, routing_with_overlays());
        assert!(
            manifest.artifact("routing/overlays/46.edgo").is_some(),
            "overlay deltas are served like any artifact"
        );
    }
}
