//! `boost.bin` -- the boost stars that are not the arrival star.
//!
//! A record's class is the arrival star's, and that is all a supercharge
//! ever came from: 3,852,215 systems arrive at a neutron star or white
//! dwarf. Another 127,246 hold one only as a secondary (full-dump scan,
//! 2026-09-10, `docs/benches/2026-09-10-boost-secondary-scan.csv`), 60,811
//! of them within 10,000 ls of arrival -- reachable by a supercruise run
//! the cost model can price. The record marks them with
//! [`crate::format::FLAG_BOOST_SECONDARY`]; this side file, keyed by
//! id64, says which class and how far, so the flag costs one byte test
//! per candidate and the lookup is paid only where it is true.
//!
//! Written by the importer beside the four index files, read on first
//! use like `alt250.bin` and `graph250.bin`, and like them not part of the
//! published product: a client without it plans as before (the flag
//! alone grants nothing), and a rebase rewrites it whole. Overlays never
//! touch it -- a secondary learnt from a scan after the rebase waits for
//! the next one.
//!
//! Layout, little-endian: `EDBS`, version u32 = 1, count u64, then
//! `count` entries of 16 bytes -- id64 u64, distance from arrival as
//! f32 light seconds, class code u8, three zero bytes -- sorted by id64.

use anyhow::{bail, Context, Result};
use memmap2::Mmap;
use std::path::Path;

pub const BOOST_SIDE_FILE: &str = "boost.bin";
const MAGIC: &[u8; 4] = b"EDBS";
const VERSION: u32 = 1;
const HEADER: usize = 16;
const ENTRY: usize = 16;

/// One boost star that is not the arrival star.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct SecondaryBoost {
    pub id64: u64,
    /// Distance from the arrival point, light seconds.
    pub ls: f32,
    /// [`crate::StarClass`] code: neutron or white dwarf.
    pub class: u8,
}

/// Write the side file. Entries are sorted here; duplicates keep the
/// nearest.
pub fn write(path: &Path, entries: &mut Vec<SecondaryBoost>) -> Result<()> {
    entries.sort_by(|a, b| a.id64.cmp(&b.id64).then(a.ls.total_cmp(&b.ls)));
    entries.dedup_by_key(|e| e.id64);
    let mut out = Vec::with_capacity(HEADER + entries.len() * ENTRY);
    out.extend_from_slice(MAGIC);
    out.extend_from_slice(&VERSION.to_le_bytes());
    out.extend_from_slice(&(entries.len() as u64).to_le_bytes());
    for e in entries.iter() {
        out.extend_from_slice(&e.id64.to_le_bytes());
        out.extend_from_slice(&e.ls.to_le_bytes());
        out.push(e.class);
        out.extend_from_slice(&[0, 0, 0]);
    }
    std::fs::write(path, out).with_context(|| format!("writing {}", path.display()))
}

/// The side file, memory-mapped; lookups are a binary search by id64.
pub struct BoostSide {
    map: Mmap,
    count: usize,
}

impl BoostSide {
    pub fn open(path: &Path) -> Result<BoostSide> {
        let f = std::fs::File::open(path).with_context(|| format!("opening {}", path.display()))?;
        // SAFETY: written whole by `write` and never modified in place.
        let map = unsafe { Mmap::map(&f)? };
        if map.len() < HEADER || &map[0..4] != MAGIC {
            bail!("{} is not a boost side file", path.display());
        }
        let version = u32::from_le_bytes(map[4..8].try_into().unwrap());
        if version != VERSION {
            bail!("boost side file version {version}, expected {VERSION}");
        }
        let count = u64::from_le_bytes(map[8..16].try_into().unwrap()) as usize;
        if map.len() < HEADER + count * ENTRY {
            bail!("{} is truncated", path.display());
        }
        Ok(BoostSide { map, count })
    }

    pub fn len(&self) -> usize {
        self.count
    }

    pub fn is_empty(&self) -> bool {
        self.count == 0
    }

    fn entry(&self, i: usize) -> SecondaryBoost {
        let o = HEADER + i * ENTRY;
        let b = &self.map[o..o + ENTRY];
        SecondaryBoost {
            id64: u64::from_le_bytes(b[0..8].try_into().unwrap()),
            ls: f32::from_le_bytes(b[8..12].try_into().unwrap()),
            class: b[12],
        }
    }

    pub fn lookup(&self, id64: u64) -> Option<SecondaryBoost> {
        let (mut lo, mut hi) = (0usize, self.count);
        while lo < hi {
            let mid = lo + (hi - lo) / 2;
            let e = self.entry(mid);
            match e.id64.cmp(&id64) {
                std::cmp::Ordering::Equal => return Some(e),
                std::cmp::Ordering::Less => lo = mid + 1,
                std::cmp::Ordering::Greater => hi = mid,
            }
        }
        None
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_side_file_round_trips_sorted_and_deduplicated() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join(BOOST_SIDE_FILE);
        let mut entries = vec![
            SecondaryBoost { id64: 30, ls: 900.0, class: 13 },
            SecondaryBoost { id64: 10, ls: 3_000.0, class: 14 },
            SecondaryBoost { id64: 30, ls: 8_000.0, class: 14 }, // farther duplicate: dropped
            SecondaryBoost { id64: 20, ls: 120_000.0, class: 14 },
        ];
        write(&path, &mut entries).unwrap();
        let side = BoostSide::open(&path).unwrap();
        assert_eq!(side.len(), 3);
        assert_eq!(side.lookup(10), Some(SecondaryBoost { id64: 10, ls: 3_000.0, class: 14 }));
        assert_eq!(side.lookup(30), Some(SecondaryBoost { id64: 30, ls: 900.0, class: 13 }));
        assert_eq!(side.lookup(20).map(|e| e.ls), Some(120_000.0));
        assert_eq!(side.lookup(25), None);
        assert_eq!(side.lookup(5), None);
        assert_eq!(side.lookup(40), None);
    }

    #[test]
    fn a_foreign_or_truncated_file_is_refused() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join(BOOST_SIDE_FILE);
        std::fs::write(&path, b"EDGX....").unwrap();
        assert!(BoostSide::open(&path).is_err());
        let mut one = vec![SecondaryBoost { id64: 1, ls: 1.0, class: 14 }];
        write(&path, &mut one).unwrap();
        let bytes = std::fs::read(&path).unwrap();
        std::fs::write(&path, &bytes[..bytes.len() - 4]).unwrap();
        assert!(BoostSide::open(&path).is_err());
    }
}
