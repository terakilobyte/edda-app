//! `scoop250.bin` -- every highway star's nearest scoopable neighbour.
//!
//! The coarse search needs one fact per highway star: can the ship refuel
//! within a plain jump of here? Until 2026-10-17 it read
//! [`crate::format::FLAG_SCOOP_NEARBY`], a companion within 1,500 ls,
//! which is true of 9.4 % of highway stars -- while 92 % have a scoopable
//! star within 80 ly and the median is 30 ly (full-index census, planner
//! unification R2). This side file carries that distance in whole light
//! years, one byte per record of the highway sub-index in record order,
//! 255 meaning none within 254 ly. Built with the sub-index from the full
//! index ([`crate::import::nearest_scoop_bytes`]); optional at run time
//! like `boost.bin`: without it the planner reads the flag as before.
//!
//! Layout, little-endian: `EDSC`, version u32 = 1, count u64, then
//! `count` bytes.

use anyhow::{bail, Context, Result};
use memmap2::Mmap;
use std::path::Path;

pub const SCOOP_SIDE_FILE: &str = "scoop250.bin";
const MAGIC: &[u8; 4] = b"EDSC";
const VERSION: u32 = 1;
const HEADER: usize = 16;
/// The byte that means "no scoopable star within 254 ly".
pub const NONE: u8 = 255;

/// Write the side file: one byte per sub-index record, in record order.
pub fn write(path: &Path, bytes: &[u8]) -> Result<()> {
    let mut out = Vec::with_capacity(HEADER + bytes.len());
    out.extend_from_slice(MAGIC);
    out.extend_from_slice(&VERSION.to_le_bytes());
    out.extend_from_slice(&(bytes.len() as u64).to_le_bytes());
    out.extend_from_slice(bytes);
    std::fs::write(path, out).with_context(|| format!("writing {}", path.display()))
}

/// The side file, memory-mapped; a lookup is one byte read.
pub struct ScoopSide {
    map: Mmap,
    count: usize,
}

impl ScoopSide {
    pub fn open(path: &Path) -> Result<ScoopSide> {
        let f = std::fs::File::open(path).with_context(|| format!("opening {}", path.display()))?;
        // SAFETY: written whole by `write` and never modified in place.
        let map = unsafe { Mmap::map(&f)? };
        if map.len() < HEADER || &map[0..4] != MAGIC {
            bail!("{} is not a scoop side file", path.display());
        }
        let version = u32::from_le_bytes(map[4..8].try_into().unwrap());
        if version != VERSION {
            bail!("scoop side file version {version}, expected {VERSION}");
        }
        let count = u64::from_le_bytes(map[8..16].try_into().unwrap()) as usize;
        if map.len() < HEADER + count {
            bail!("{} is truncated", path.display());
        }
        Ok(ScoopSide { map, count })
    }

    pub fn len(&self) -> usize {
        self.count
    }

    pub fn is_empty(&self) -> bool {
        self.count == 0
    }

    /// Nearest scoopable star of record `idx`, whole light years; `NONE`
    /// (255) when nothing within 254 ly; `None` past the end of the file.
    pub fn get(&self, idx: u32) -> Option<u8> {
        let i = idx as usize;
        (i < self.count).then(|| self.map[HEADER + i])
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_scoop_file_round_trips_and_answers_by_record_index() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join(SCOOP_SIDE_FILE);
        write(&path, &[30, 255, 0, 72]).unwrap();
        let side = ScoopSide::open(&path).unwrap();
        assert_eq!(side.len(), 4);
        assert_eq!(side.get(0), Some(30));
        assert_eq!(side.get(1), Some(NONE), "255 is none within 254 ly");
        assert_eq!(side.get(3), Some(72));
        assert_eq!(side.get(4), None, "past the end: unknown");
    }

    #[test]
    fn a_foreign_or_truncated_file_is_refused() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join(SCOOP_SIDE_FILE);
        std::fs::write(&path, b"EDBS\x01\x00\x00\x00\x00\x00\x00\x00\x00\x00\x00\x00").unwrap();
        assert!(ScoopSide::open(&path).is_err(), "a boost file is not a scoop file");
        let mut good = Vec::new();
        good.extend_from_slice(MAGIC);
        good.extend_from_slice(&VERSION.to_le_bytes());
        good.extend_from_slice(&9u64.to_le_bytes());
        good.extend_from_slice(&[1, 2, 3]);
        std::fs::write(&path, good).unwrap();
        assert!(ScoopSide::open(&path).is_err(), "nine promised, three present");
    }
}
