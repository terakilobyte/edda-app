//! The sync protocol both the publisher and the desktop client share: the
//! versioned manifest, SHA-256 verification and the resumable-download
//! policy. The EBEX container format itself lives in `ed-ebex`.

pub mod chunks;
pub mod digest;
pub mod download;
pub mod manifest;

pub use chunks::{Chunk, ChunkManifest, ChunkedFile, CHUNKS_FILE};
pub use digest::{is_sha256_hex, sha256_hex, sha256_hex_reader, verify_sha256_reader};
pub use manifest::{
    ArtifactFile, Manifest, OverlayLink, Product, ProductKey, SelectedArtifact, API_V1_PREFIX,
    ARTIFACT_ROUTE_PREFIX, COMMUNITY_SCHEMA_V1, MANIFEST_PROTOCOL_V1, MANIFEST_ROUTE,
    STARS_SCHEMA_V1,
};
