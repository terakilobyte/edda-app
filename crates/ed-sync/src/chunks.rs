//! Chunk manifests — the wire-unification half of ledger item 47.
//!
//! The maintainer's model: the client keeps the chunk hashes of what it last
//! synced; "if we don't have any local hashes, we know we need to
//! download it all." One sync algorithm, three cases — no inventory
//! means fetch every chunk (fresh install), a walkable chain means EDGO
//! overlays, anything else (rebase, flag-day, corruption) means fetch
//! only the chunks the inventory lacks. The whole-file download path
//! dissolves into the degenerate first case.
//!
//! A `ChunkManifest` describes one published version's files as CDC
//! chunk lists. It is published as its own small artifact beside the
//! full files (`chunks.json` in the version directory, listed in the
//! product's files so it verifies like anything else). Boundaries are
//! content-defined (FastCDC) because fixed-grid chunking measured ~zero
//! reuse across rebases (docs/benches/2026-09-03-item47-cdc-delta.csv)
//! while CDC measured 36–76% on same-format rebases.

use anyhow::{ensure, Result};
use serde::{Deserialize, Serialize};

use crate::digest::is_sha256_hex;

/// File name of the chunk manifest inside a published version directory.
pub const CHUNKS_FILE: &str = "chunks.json";

/// One contiguous piece of one published file.
#[derive(Clone, Debug, PartialEq, Eq, Deserialize, Serialize)]
pub struct Chunk {
    pub offset: u64,
    pub len: u32,
    /// Lowercase hex SHA-256 of the chunk's bytes.
    pub sha256: String,
}

/// One file as a chunk list. `sha256` is the whole file's digest and must
/// match the product's `ArtifactFile` for the same path.
#[derive(Clone, Debug, PartialEq, Eq, Deserialize, Serialize)]
pub struct ChunkedFile {
    pub name: String,
    pub bytes: u64,
    pub sha256: String,
    pub chunks: Vec<Chunk>,
}

#[derive(Clone, Debug, PartialEq, Eq, Deserialize, Serialize)]
pub struct ChunkManifest {
    /// The product version these chunks describe.
    pub version: String,
    pub files: Vec<ChunkedFile>,
}

impl ChunkManifest {
    pub fn validate(&self) -> Result<()> {
        ensure!(!self.version.trim().is_empty(), "chunk manifest version is empty");
        ensure!(!self.files.is_empty(), "chunk manifest lists no files");
        for file in &self.files {
            file.validate()?;
        }
        Ok(())
    }

    /// Every chunk hash the manifest names — the set a client diffs its
    /// inventory against. `(file name, chunk)` pairs in file order.
    pub fn all_chunks(&self) -> impl Iterator<Item = (&str, &Chunk)> {
        self.files
            .iter()
            .flat_map(|file| file.chunks.iter().map(move |chunk| (file.name.as_str(), chunk)))
    }
}

impl ChunkedFile {
    pub fn validate(&self) -> Result<()> {
        ensure!(!self.name.trim().is_empty(), "chunked file has no name");
        ensure!(is_sha256_hex(&self.sha256), "chunked file {} has an invalid sha256", self.name);
        ensure!(!self.chunks.is_empty() || self.bytes == 0, "chunked file {} has no chunks", self.name);
        let mut at = 0u64;
        for chunk in &self.chunks {
            ensure!(
                chunk.offset == at,
                "chunked file {}: chunk at offset {} expected {at} (gap or overlap)",
                self.name,
                chunk.offset
            );
            ensure!(chunk.len > 0, "chunked file {}: empty chunk at {at}", self.name);
            ensure!(is_sha256_hex(&chunk.sha256), "chunked file {}: invalid chunk sha256 at {at}", self.name);
            at += u64::from(chunk.len);
        }
        ensure!(
            at == self.bytes,
            "chunked file {}: chunks cover {at} of {} bytes",
            self.name,
            self.bytes
        );
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn chunk(offset: u64, len: u32) -> Chunk {
        Chunk { offset, len, sha256: "0".repeat(64) }
    }

    fn manifest() -> ChunkManifest {
        ChunkManifest {
            version: "51".into(),
            files: vec![ChunkedFile {
                name: "stars.bin".into(),
                bytes: 300,
                sha256: "0".repeat(64),
                chunks: vec![chunk(0, 100), chunk(100, 200)],
            }],
        }
    }

    #[test]
    fn a_contiguous_manifest_validates_and_round_trips() {
        let m = manifest();
        m.validate().unwrap();
        let json = serde_json::to_string(&m).unwrap();
        assert_eq!(serde_json::from_str::<ChunkManifest>(&json).unwrap(), m);
        assert_eq!(m.all_chunks().count(), 2);
    }

    #[test]
    fn gaps_overlaps_and_short_coverage_are_refused() {
        let mut gap = manifest();
        gap.files[0].chunks[1].offset = 150;
        assert!(gap.validate().is_err(), "gap");

        let mut short = manifest();
        short.files[0].bytes = 400;
        assert!(short.validate().is_err(), "short coverage");

        let mut empty = manifest();
        empty.files[0].chunks.clear();
        assert!(empty.validate().is_err(), "no chunks for a non-empty file");

        let mut zero = manifest();
        zero.files[0].bytes = 0;
        zero.files[0].chunks.clear();
        zero.validate().expect("an empty file has no chunks and that is fine");
    }
}
