//! `present.bin` -- which cells of a v3 index actually hold their bytes.
//!
//! The design-B chunked download (ROUTING-NEXT item 9) fetches
//! `stars.bin`/`names.bin` as ranged cell-group requests instead of
//! whole files, nearest-first. While that download is under way the
//! index on disk is sparse: `cells.bin` and `byname.bin` are whole (they
//! are the index's skeleton and arrive first), but a cell's record and
//! name bytes may not have landed yet -- those ranges read as zeros, not
//! as data. This sidecar is the reader's truth about which cells are
//! real: one bit per occupied cell, in the cell array's (morton) order.
//!
//! Absence of the file = a whole-file install, everything present --
//! today's downloads never write one, so nothing changes for them. The
//! fetcher creates the file all-absent when it starts a sparse install
//! and flips bits as ranges verify; readers skip absent cells in every
//! spatial query and refuse to read their records as names (a zeroed
//! record would otherwise silently corrupt a binary search).

use std::io::Write as _;
use std::path::Path;

use anyhow::{bail, ensure, Context, Result};

/// File name beside `cells.bin`.
pub const PRESENT_FILE: &str = "present.bin";
const MAGIC: &[u8; 4] = b"PRES";

pub struct Presence {
    bits: Vec<u8>,
    cells: usize,
    present: usize,
}

impl Presence {
    /// A fresh sparse install: every cell absent.
    pub fn new_absent(cells: usize) -> Presence {
        Presence { bits: vec![0; cells.div_ceil(8)], cells, present: 0 }
    }

    pub fn cells(&self) -> usize {
        self.cells
    }

    pub fn present(&self) -> usize {
        self.present
    }

    pub fn missing(&self) -> usize {
        self.cells - self.present
    }

    pub fn is_present(&self, cell: usize) -> bool {
        cell < self.cells && self.bits[cell / 8] & (1 << (cell % 8)) != 0
    }

    /// Flip one cell to present (a verified range landed). Idempotent.
    pub fn mark_present(&mut self, cell: usize) {
        assert!(cell < self.cells, "cell {cell} out of {}", self.cells);
        let (byte, bit) = (cell / 8, 1u8 << (cell % 8));
        if self.bits[byte] & bit == 0 {
            self.bits[byte] |= bit;
            self.present += 1;
        }
    }

    pub fn write(&self, path: &Path) -> Result<()> {
        let mut out = Vec::with_capacity(12 + self.bits.len());
        out.extend_from_slice(MAGIC);
        out.extend_from_slice(&1u32.to_le_bytes());
        out.extend_from_slice(&(self.cells as u64).to_le_bytes());
        out.extend_from_slice(&self.bits);
        let mut file = std::fs::File::create(path).with_context(|| format!("writing {}", path.display()))?;
        file.write_all(&out)?;
        Ok(())
    }

    pub fn open(path: &Path) -> Result<Presence> {
        let bytes = std::fs::read(path).with_context(|| format!("reading {}", path.display()))?;
        if bytes.len() < 16 || &bytes[0..4] != MAGIC {
            bail!("{} is not a PRES presence bitmap", path.display());
        }
        let version = u32::from_le_bytes(bytes[4..8].try_into().unwrap());
        ensure!(version == 1, "unknown presence version {version}");
        let cells = u64::from_le_bytes(bytes[8..16].try_into().unwrap()) as usize;
        let bits = bytes[16..].to_vec();
        ensure!(bits.len() == cells.div_ceil(8), "{} holds {} bitmap bytes for {cells} cells", path.display(), bits.len());
        let present = bits.iter().map(|b| b.count_ones() as usize).sum();
        ensure!(present <= cells, "{} marks more cells present than exist", path.display());
        Ok(Presence { bits, cells, present })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn bits_round_trip_and_count() {
        let dir = tempfile::tempdir().unwrap();
        let mut p = Presence::new_absent(11);
        assert_eq!((p.cells(), p.present(), p.missing()), (11, 0, 11));
        p.mark_present(0);
        p.mark_present(10);
        p.mark_present(10); // idempotent
        assert_eq!(p.present(), 2);
        assert!(p.is_present(0) && p.is_present(10) && !p.is_present(5));
        assert!(!p.is_present(11), "out of range is absent, not a panic");
        let path = dir.path().join(PRESENT_FILE);
        p.write(&path).unwrap();
        let q = Presence::open(&path).unwrap();
        assert_eq!((q.cells(), q.present()), (11, 2));
        assert!(q.is_present(10) && !q.is_present(9));
    }

    #[test]
    fn open_rejects_wrong_magic_and_truncation() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join(PRESENT_FILE);
        std::fs::write(&path, b"nope").unwrap();
        assert!(Presence::open(&path).is_err());
        let mut p = Vec::new();
        p.extend_from_slice(b"PRES");
        p.extend_from_slice(&1u32.to_le_bytes());
        p.extend_from_slice(&100u64.to_le_bytes());
        p.push(0); // 100 cells need 13 bytes
        std::fs::write(&path, &p).unwrap();
        assert!(Presence::open(&path).is_err());
    }
}
