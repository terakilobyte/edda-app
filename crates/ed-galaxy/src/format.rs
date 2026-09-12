//! The on-disk star index, memory-mapped.
//!
//! Four files in one directory, all little-endian, all written by
//! [`crate::import`] and never modified in place:
//!
//! * `stars.bin`   -- 32-byte header, then one packed [`StarRecord`] (29 bytes) per
//!                    system, sorted by grid cell so a cell is one contiguous
//!                    slice.
//! * `cells.bin`   -- sorted `(cell_key u64, start u32, count u32)` entries:
//!                    the spatial index.
//! * `names.bin`   -- UTF-8 names, addressed by (offset, len) from the record.
//! * `byname.bin`  -- `u32` record indices sorted by lower-cased name, for
//!                    binary-search lookup.
//!
//! 150M systems come to ~4.8 GB of records and ~3 GB of names; the OS pages
//! in what a query touches and nothing else.

use crate::star::StarClassCode as _;
use anyhow::{bail, Context, Result};
use memmap2::Mmap;
use std::fs::File;
use std::path::{Path, PathBuf};

pub const MAGIC: &[u8; 4] = b"EDGX";
pub const VERSION: u32 = 3;
pub const HEADER_LEN: usize = 32;
pub const RECORD_LEN: usize = 29;
const V1_RECORD_LEN: usize = 32;
pub const CELL_LEN: usize = 16;
/// Record flag bits.
pub const FLAG_MAIN_STAR: u8 = 0x01;
/// A scoopable star sits within [`SCOOP_COMPANION_LS`] of the arrival
/// point -- a neutron system you can refuel in without leaving the highway.
pub const FLAG_SCOOP_NEARBY: u8 = 0x02;
/// A neutron star or white dwarf sits in the system but is not the
/// arrival star; which class and how far is in `boost.bin`
/// ([`crate::boost_side`]). Set at import from the dump's body list;
/// the record class stays the arrival star's, so nothing that reads the
/// class changes. (2026-09-10: 127,246 such systems, 60,811 within
/// 10,000 ls.)
pub const FLAG_BOOST_SECONDARY: u8 = 0x04;
pub const SCOOP_COMPANION_LS: f64 = 1_500.0;
/// Grid cell edge in light years. Larger than any single jump, so a
/// neighbourhood query touches at most 27 cells.
pub const CELL_LY: f32 = 50.0;

/// One decoded system. Version 2 packs this into 29 bytes on disk, including
/// one reserved metadata byte so future background migrations need not
/// redownload or reparse the source dump.
#[derive(Debug, Clone, Copy, PartialEq)]
#[repr(C)]
pub struct StarRecord {
    pub x: f32,
    pub y: f32,
    pub z: f32,
    pub class: u8,
    pub flags: u8,
    /// Companion-distance bucket (v3; 0 on records written before v3).
    pub companion: u8,
    pub name_len: u16,
    pub id64: u64,
    pub name_off: u64,
}

impl StarRecord {
    pub fn pos(&self) -> [f32; 3] {
        [self.x, self.y, self.z]
    }

    pub fn write_to(&self, out: &mut Vec<u8>) {
        out.extend_from_slice(&self.x.to_le_bytes());
        out.extend_from_slice(&self.y.to_le_bytes());
        out.extend_from_slice(&self.z.to_le_bytes());
        debug_assert!(self.class <= 0x0f && self.flags <= 0x0f);
        out.push(self.class | (self.flags << 4));
        debug_assert!(self.companion <= COMPANION_BUCKETS);
        out.push(self.companion);
        out.extend_from_slice(&self.name_len.to_le_bytes());
        out.extend_from_slice(&self.id64.to_le_bytes());
        debug_assert!(self.name_off < (1u64 << 40));
        out.extend_from_slice(&self.name_off.to_le_bytes()[..5]);
    }

    pub fn read_from(b: &[u8]) -> Self {
        let f = |i: usize| f32::from_le_bytes([b[i], b[i + 1], b[i + 2], b[i + 3]]);
        StarRecord {
            x: f(0),
            y: f(4),
            z: f(8),
            class: b[12] & 0x0f,
            flags: b[12] >> 4,
            companion: b[13],
            name_len: u16::from_le_bytes([b[14], b[15]]),
            id64: u64::from_le_bytes(b[16..24].try_into().unwrap()),
            name_off: u64::from_le_bytes([b[24], b[25], b[26], b[27], b[28], 0, 0, 0]),
        }
    }

    fn read_v1(b: &[u8]) -> Self {
        let f = |i: usize| f32::from_le_bytes([b[i], b[i + 1], b[i + 2], b[i + 3]]);
        StarRecord {
            x: f(0),
            y: f(4),
            z: f(8),
            class: b[12],
            flags: b[13],
            companion: 0,
            name_len: u16::from_le_bytes([b[14], b[15]]),
            id64: u64::from_le_bytes(b[16..24].try_into().unwrap()),
            name_off: u64::from_le_bytes(b[24..32].try_into().unwrap()),
        }
    }
}

/// Pack a cell coordinate triple into a sortable key: the v1/v2 layout,
/// axes concatenated (x major). Kept to read those indexes.
pub fn cell_key(cx: i32, cy: i32, cz: i32) -> u64 {
    let p = |v: i32| ((v + (1 << 20)) as u64) & 0x1F_FFFF;
    (p(cx) << 42) | (p(cy) << 21) | p(cz)
}

/// The v3 cell key: the same 21-bit biased coordinates, bit-interleaved
/// (Morton / z-order). Cells close in space sort close in the file, so a
/// locality-first download fetches a neighbourhood as one contiguous run
/// instead of three scattered stripes.
pub fn morton_cell_key(cx: i32, cy: i32, cz: i32) -> u64 {
    let p = |v: i32| ((v + (1 << 20)) as u64) & 0x1F_FFFF;
    spread21(p(cx)) << 2 | spread21(p(cy)) << 1 | spread21(p(cz))
}

/// Spread the low 21 bits of `v` so each lands three positions apart
/// (…b2 0 0 b1 0 0 b0): the standard mask-and-shift Morton dilation.
fn spread21(v: u64) -> u64 {
    let mut v = v & 0x1F_FFFF;
    v = (v | (v << 32)) & 0x1F_0000_0000_FFFF;
    v = (v | (v << 16)) & 0x1F_0000_FF00_00FF;
    v = (v | (v << 8)) & 0x100F_00F0_0F00_F00F;
    v = (v | (v << 4)) & 0x10C3_0C30_C30C_30C3;
    (v | (v << 2)) & 0x1249_2492_4924_9249
}

/// Undo [`spread21`]: gather every third bit back into the low 21.
fn compact21(v: u64) -> u64 {
    let mut v = v & 0x1249_2492_4924_9249;
    v = (v | (v >> 2)) & 0x10C3_0C30_C30C_30C3;
    v = (v | (v >> 4)) & 0x100F_00F0_0F00_F00F;
    v = (v | (v >> 8)) & 0x1F_0000_FF00_00FF;
    v = (v | (v >> 16)) & 0x1F_0000_0000_FFFF;
    (v | (v >> 32)) & 0x1F_FFFF
}

/// Cell coordinates of a v3 Morton key: the inverse of [`morton_cell_key`].
pub fn morton_cell_of(key: u64) -> (i32, i32, i32) {
    let un = |v: u64| (v as i64 - (1 << 20)) as i32;
    (
        un(compact21(key >> 2)),
        un(compact21(key >> 1)),
        un(compact21(key)),
    )
}

/// The axis a bit position belongs to, as its dilated mask: every bit of
/// that axis across the whole key.
fn axis_mask(bit: u32) -> u64 {
    0x1249_2492_4924_9249u64 << (bit % 3)
}

/// BIGMIN (Tropf & Herzog): the smallest Morton key whose cell lies in
/// the box spanned by the cells of `zmin`..`zmax` (per-axis min and max
/// corners) and which is strictly greater than `key`. `None` when the box
/// holds no key beyond `key`. This is what lets a range walk over
/// morton-sorted cells skip the out-of-box gaps in one jump instead of
/// probing every cell of the bounding box.
pub fn morton_bigmin(key: u64, mut zmin: u64, mut zmax: u64) -> Option<u64> {
    let mut bigmin = None;
    for bit in (0..63u32).rev() {
        let mask = 1u64 << bit;
        // In this axis, take the branch above `bit` (set it, clear the
        // axis bits below) or below it (clear it, saturate the ones below).
        let above = |v: u64| (v & !(axis_mask(bit) & (mask - 1))) | mask;
        let below = |v: u64| (v & !mask) | (axis_mask(bit) & (mask - 1));
        match (key & mask != 0, zmin & mask != 0, zmax & mask != 0) {
            (false, false, false) => {}
            (false, false, true) => {
                bigmin = Some(above(zmin));
                zmax = below(zmax);
            }
            (false, true, true) => return Some(zmin),
            (true, false, false) => return bigmin,
            (true, false, true) => {
                zmin = above(zmin);
            }
            (true, true, true) => {}
            // A box's min corner cannot exceed its max corner in any axis.
            (_, true, false) => unreachable!("inverted Morton box"),
        }
    }
    bigmin
}

/// Companion-distance buckets (v3, record byte 13). 0 = no scoopable
/// companion within [`SCOOP_COMPANION_LS`] (or a pre-v3 record); 1..=63 is
/// a log-scale bucket over (0, 1500] ls, so the follow-mode time estimate
/// can tell a 40 ls hop from a 1,400 ls supercruise.
pub const COMPANION_BUCKETS: u8 = 63;

/// Bucket for a companion at `ls` light-seconds (clamped into range).
pub fn companion_bucket(ls: f64) -> u8 {
    let clamped = ls.clamp(1.0, SCOOP_COMPANION_LS);
    let scaled = (clamped.ln() / SCOOP_COMPANION_LS.ln()) * f64::from(COMPANION_BUCKETS - 1);
    1 + (scaled.floor() as u8).min(COMPANION_BUCKETS - 1)
}

/// The distance a bucket stands for (its bucket's geometric midpoint);
/// `None` for 0 (no companion known).
pub fn companion_bucket_ls(bucket: u8) -> Option<f64> {
    if bucket == 0 || bucket > COMPANION_BUCKETS {
        return None;
    }
    let mid = (f64::from(bucket - 1) + 0.5) / f64::from(COMPANION_BUCKETS - 1);
    Some(SCOOP_COMPANION_LS.powf(mid).min(SCOOP_COMPANION_LS))
}

pub fn cell_of(pos: [f32; 3]) -> (i32, i32, i32) {
    cell_of_with(pos, CELL_LY)
}

/// Cell of `pos` for an index built with `cell_ly` cells.
pub fn cell_of_with(pos: [f32; 3], cell_ly: f32) -> (i32, i32, i32) {
    (
        (pos[0] / cell_ly).floor() as i32,
        (pos[1] / cell_ly).floor() as i32,
        (pos[2] / cell_ly).floor() as i32,
    )
}

pub fn dist(a: [f32; 3], b: [f32; 3]) -> f32 {
    ((a[0] - b[0]).powi(2) + (a[1] - b[1]).powi(2) + (a[2] - b[2]).powi(2)).sqrt()
}

/// The opened index.
pub struct Galaxy {
    stars: Mmap,
    cells: Mmap,
    names: Mmap,
    byname: Mmap,
    pub count: usize,
    pub dir: PathBuf,
    /// Grid cell edge this index was built with (header; 50 ly by default,
    /// larger for sparse sub-indexes so a 500 ly query is not 8,000 lookups).
    pub cell_ly: f32,
    version: u32,
    record_len: usize,
    /// Star classes learned after the index was built (Spansh /system,
    /// the commander's own scans), keyed by id64. The index files are
    /// immutable; this is the writable layer over them.
    overrides: std::sync::RwLock<std::collections::HashMap<u64, u8>>,
    /// The prefix-aggregate oracle (`agg250.bin`), opened on first use.
    /// `None` inside: the file is absent or unreadable -- oracle off.
    aggregate: std::sync::OnceLock<Option<crate::agg::Aggregate>>,
    /// Which cells' record/name bytes have actually landed
    /// (`present.bin`, written by a sparse chunked install), opened on
    /// first use. `None` inside: no bitmap -- a whole-file install,
    /// everything present.
    presence: std::sync::OnceLock<Option<crate::presence::Presence>>,
    /// The ALT landmark table (`alt250.bin`), opened on first use.
    /// `None` inside: absent or stale -- straight-line heuristic only.
    alt: std::sync::OnceLock<Option<crate::alt::AltOracle>>,
    /// The coarse cell graph (`graph250.bin`), opened on first use.
    /// `None` inside: absent or stale -- no goal field for the plot.
    cell_graph: std::sync::OnceLock<Option<crate::cgraph::CellGraph>>,
    /// The secondary boost stars (`boost.bin`), opened on first use.
    /// `None` inside: absent -- [`Galaxy::boost_secondary`] is never Some.
    boost_side: std::sync::OnceLock<Option<crate::boost_side::BoostSide>>,
}

impl Galaxy {
    pub fn paths(dir: &Path) -> [PathBuf; 4] {
        [
            dir.join("stars.bin"),
            dir.join("cells.bin"),
            dir.join("names.bin"),
            dir.join("byname.bin"),
        ]
    }

    pub fn exists(dir: &Path) -> bool {
        Self::paths(dir).iter().all(|p| p.is_file())
    }

    pub fn open(dir: &Path) -> Result<Self> {
        let [s, c, n, b] = Self::paths(dir);
        let map = |p: &Path| -> Result<Mmap> {
            let f = File::open(p).with_context(|| format!("opening {}", p.display()))?;
            // SAFETY: the files are written once and never modified in place.
            Ok(unsafe { Mmap::map(&f)? })
        };
        let stars = map(&s)?;
        if stars.len() < HEADER_LEN || &stars[0..4] != MAGIC {
            bail!("{} is not a galaxy index", s.display());
        }
        let version = u32::from_le_bytes(stars[4..8].try_into().unwrap());
        if !(1..=VERSION).contains(&version) {
            bail!("galaxy index version {version}, expected 1 to {VERSION}");
        }
        let record_len = if version == 1 {
            V1_RECORD_LEN
        } else {
            RECORD_LEN
        };
        let count = u64::from_le_bytes(stars[8..16].try_into().unwrap()) as usize;
        if stars.len() < HEADER_LEN + count * record_len {
            bail!("{} is truncated", s.display());
        }
        let cell_ly = f32::from_le_bytes(stars[16..20].try_into().unwrap());
        let cell_ly = if cell_ly.is_finite() && cell_ly > 0.0 {
            cell_ly
        } else {
            CELL_LY
        };
        Ok(Galaxy {
            stars,
            cells: map(&c)?,
            names: map(&n)?,
            byname: map(&b)?,
            count,
            dir: dir.to_path_buf(),
            cell_ly,
            version,
            record_len,
            overrides: Default::default(),
            aggregate: Default::default(),
            presence: Default::default(),
            alt: Default::default(),
            cell_graph: Default::default(),
            boost_side: Default::default(),
        })
    }

    /// The boost star that is not this system's arrival star, with its
    /// distance from arrival in light seconds: only for records flagged
    /// [`FLAG_BOOST_SECONDARY`], and only where `boost.bin` sits beside
    /// the index. The planner reads it when a request allows a secondary
    /// within some distance; otherwise the flag grants nothing.
    pub fn boost_secondary(&self, idx: u32) -> Option<(crate::StarClass, f32)> {
        if self.flags(idx) & FLAG_BOOST_SECONDARY == 0 {
            return None;
        }
        let side = self
            .boost_side
            .get_or_init(|| {
                crate::boost_side::BoostSide::open(
                    &self.dir.join(crate::boost_side::BOOST_SIDE_FILE),
                )
                .ok()
            })
            .as_ref()?;
        let e = side.lookup(self.record(idx).id64)?;
        Some((crate::StarClass::from_code(e.class), e.ls))
    }

    /// The coarse cell graph, if `graph250.bin` was written beside the
    /// index (see [`crate::cgraph`]). Stale files (wrong cell count)
    /// are ignored.
    pub fn cell_graph(&self) -> Option<&crate::cgraph::CellGraph> {
        self.cell_graph
            .get_or_init(|| {
                crate::cgraph::CellGraph::open(&self.dir.join(crate::cgraph::GRAPH_FILE))
                    .ok()
                    .filter(|graph| graph.leaf_count() == self.cell_count())
            })
            .as_ref()
    }

    /// The presence bitmap of a sparse (chunked) install, if one exists.
    /// A bitmap sized for a different cell array is not this index's and
    /// is ignored -- which fails SAFE only because the fetcher writes
    /// the bitmap before any record bytes: a mismatched file means a
    /// finished whole-file install replaced the sparse one.
    pub fn presence(&self) -> Option<&crate::presence::Presence> {
        self.presence
            .get_or_init(|| {
                crate::presence::Presence::open(&self.dir.join(crate::presence::PRESENT_FILE))
                    .ok()
                    .filter(|p| p.cells() == self.cell_count())
            })
            .as_ref()
    }

    /// Is cell `i` of the cell array real data (or a hole a sparse
    /// download has not filled yet)?
    fn cell_data_present(&self, i: usize) -> bool {
        self.presence().is_none_or(|p| p.is_present(i))
    }

    /// The record's cell, by its position in the record space (records
    /// are stored in cell order, so cell starts are ascending).
    fn cell_of_record(&self, idx: u32) -> usize {
        let cells = self.cell_count();
        let (mut lo, mut hi) = (0usize, cells);
        while lo < hi {
            let mid = (lo + hi) / 2;
            if self.cell_entry(mid).1 <= idx {
                lo = mid + 1;
            } else {
                hi = mid;
            }
        }
        lo.saturating_sub(1)
    }

    /// Occupied cells in the box whose bytes have NOT landed yet -- what
    /// a chunked fetcher still owes this volume. Empty when the index
    /// has no presence bitmap (whole-file install).
    pub fn missing_cells_in_box(
        &self,
        lo: (i32, i32, i32),
        hi: (i32, i32, i32),
    ) -> Vec<(i32, i32, i32)> {
        let Some(presence) = self.presence() else {
            return Vec::new();
        };
        if presence.missing() == 0 || self.version < 3 {
            // Sparse installs are a v3 product feature; a bitmap on a
            // pre-morton index has no defined cell decoding.
            return Vec::new();
        }
        // Presence filtering hides absent cells from every query, so
        // enumerate the raw cell array directly. Linear over occupied
        // cells (~500 k on the boost tier, a few ms) -- the fetcher asks
        // per priority volume, not per expansion.
        let mut missing = Vec::new();
        for i in 0..self.cell_count() {
            if presence.is_present(i) {
                continue;
            }
            let (key, _, _) = self.cell_entry(i);
            let (cx, cy, cz) = morton_cell_of(key);
            if (lo.0..=hi.0).contains(&cx)
                && (lo.1..=hi.1).contains(&cy)
                && (lo.2..=hi.2).contains(&cz)
            {
                missing.push((cx, cy, cz));
            }
        }
        missing
    }

    /// The index's prefix-aggregate oracle, if `agg250.bin` was written
    /// beside it (sub-index builds write one; see [`crate::agg`]). A
    /// file whose leaf level does not match this index's cell array is
    /// stale (left by an older build) and is ignored.
    pub fn aggregate(&self) -> Option<&crate::agg::Aggregate> {
        self.aggregate
            .get_or_init(|| {
                crate::agg::Aggregate::open(&self.dir.join(crate::agg::AGG_FILE))
                    .ok()
                    .filter(|agg| agg.leaf_count() == self.cell_count())
            })
            .as_ref()
    }

    /// Perform the full, linear-time validation required before publishing an
    /// index. Normal client opens deliberately avoid this galaxy-sized scan.
    pub fn validate(&self) -> Result<()> {
        if self.version != 2 && self.version != VERSION {
            bail!("publication validation requires EDGX version 2 or {VERSION}");
        }
        let expected_stars = HEADER_LEN
            .checked_add(
                self.count
                    .checked_mul(self.record_len)
                    .context("star file size overflow")?,
            )
            .context("star file size overflow")?;
        if self.stars.len() != expected_stars {
            bail!("stars.bin has trailing or missing bytes");
        }
        if self.stars[20..HEADER_LEN].iter().any(|byte| *byte != 0) {
            bail!("stars.bin has nonzero reserved header bytes");
        }
        if !self.cells.len().is_multiple_of(CELL_LEN) {
            bail!("cells.bin length is not a multiple of {CELL_LEN}");
        }
        if self.byname.len()
            != self
                .count
                .checked_mul(4)
                .context("name index size overflow")?
        {
            bail!("byname.bin length does not equal star count times four");
        }

        let mut expected_name_off = 0u64;
        for index in 0..self.count {
            let at = HEADER_LEN + index * self.record_len;
            let bytes = &self.stars[at..at + self.record_len];
            if self.version == 2 && bytes[13] != 0 {
                bail!("star record {index} has nonzero reserved metadata");
            }
            if self.version >= 3 && bytes[13] > COMPANION_BUCKETS {
                bail!("star record {index} has an out-of-range companion bucket");
            }
            let record = StarRecord::read_from(bytes);
            if !record.x.is_finite() || !record.y.is_finite() || !record.z.is_finite() {
                bail!("star record {index} has non-finite coordinates");
            }
            if self.version >= 3 {
                // v3's ordering guarantee: names sit in record order, back
                // to back, so a cell's names are one contiguous span and a
                // ranged fetch of records maps to a ranged fetch of names.
                if record.name_off != expected_name_off {
                    bail!("star record {index} breaks the contiguous name layout");
                }
                expected_name_off += u64::from(record.name_len);
            }
            let name_start =
                usize::try_from(record.name_off).context("name offset does not fit usize")?;
            let name_end = name_start
                .checked_add(usize::from(record.name_len))
                .context("name range overflow")?;
            let name = self
                .names
                .get(name_start..name_end)
                .context("star name outside names.bin")?;
            std::str::from_utf8(name).context("invalid UTF-8 in names.bin")?;
        }
        if self.version >= 3 && expected_name_off != self.names.len() as u64 {
            bail!("names.bin has bytes no record refers to");
        }

        let mut expected_start = 0usize;
        let mut previous_key = None;
        for cell_index in 0..self.cell_count() {
            let (key, start, count) = self.cell_entry(cell_index);
            if count == 0 {
                bail!("cell record {cell_index} has zero members");
            }
            if previous_key.is_some_and(|previous| key <= previous) {
                bail!("cells.bin keys are not strictly ascending");
            }
            previous_key = Some(key);
            if start as usize != expected_start {
                bail!("cell ranges overlap or leave a gap at record {expected_start}");
            }
            let end = expected_start
                .checked_add(count as usize)
                .context("cell range overflow")?;
            if end > self.count {
                bail!("cell range extends beyond stars.bin");
            }
            for star_index in expected_start..end {
                let position = self.pos_of(star_index as u32);
                let (cx, cy, cz) = cell_of_with(position, self.cell_ly);
                if !(-1_048_576..=1_048_575).contains(&cx)
                    || !(-1_048_576..=1_048_575).contains(&cy)
                    || !(-1_048_576..=1_048_575).contains(&cz)
                    || self.key_of(cx, cy, cz) != key
                {
                    bail!("star record {star_index} is in the wrong spatial cell");
                }
            }
            expected_start = end;
        }
        if expected_start != self.count {
            bail!("cells.bin does not cover every star record");
        }

        let mut seen = vec![false; self.count];
        let mut previous: Option<(String, u32)> = None;
        for position in 0..self.count {
            let at = position * 4;
            let index = u32::from_le_bytes(self.byname[at..at + 4].try_into().unwrap());
            let slot = seen
                .get_mut(index as usize)
                .context("byname.bin index outside stars.bin")?;
            if *slot {
                bail!("byname.bin contains duplicate star index {index}");
            }
            *slot = true;
            let record = self.record(index);
            let folded = self.name(&record).to_lowercase();
            if previous.as_ref().is_some_and(|(name, prior_index)| {
                (folded.as_str(), index) <= (name.as_str(), *prior_index)
            }) {
                bail!("byname.bin is not sorted at entry {position}");
            }
            previous = Some((folded, index));
        }
        Ok(())
    }

    pub fn validate_dir(dir: &Path) -> Result<()> {
        Self::open(dir)?.validate()
    }

    pub fn record(&self, idx: u32) -> StarRecord {
        let o = HEADER_LEN + idx as usize * self.record_len;
        let bytes = &self.stars[o..o + self.record_len];
        if self.version == 1 {
            StarRecord::read_v1(bytes)
        } else {
            StarRecord::read_from(bytes)
        }
    }

    pub fn name(&self, r: &StarRecord) -> &str {
        let s = r.name_off as usize;
        std::str::from_utf8(&self.names[s..s + r.name_len as usize]).unwrap_or("?")
    }

    pub fn class(&self, r: &StarRecord) -> crate::StarClass {
        if r.class == crate::StarClass::Unknown.code() {
            if let Some(&c) = self
                .overrides
                .read()
                .unwrap_or_else(|e| e.into_inner())
                .get(&r.id64)
            {
                return crate::StarClass::from_code(c);
            }
        }
        crate::StarClass::from_code(r.class)
    }

    /// Record a star class learned elsewhere for a system the index has as
    /// unknown. Only fills gaps; it never contradicts the index.
    pub fn learn_class(&self, id64: u64, class: crate::StarClass) {
        self.overrides
            .write()
            .unwrap_or_else(|e| e.into_inner())
            .insert(id64, class.code());
    }

    pub fn learned(&self) -> usize {
        self.overrides
            .read()
            .unwrap_or_else(|e| e.into_inner())
            .len()
    }

    pub(crate) fn cell_count(&self) -> usize {
        self.cells.len() / CELL_LEN
    }

    /// Written v3+ with morton cell keys (the invariant the aggregate
    /// levels and the box walk are built on).
    pub(crate) fn morton_cells(&self) -> bool {
        self.version >= 3
    }

    pub(crate) fn cell_entry(&self, i: usize) -> (u64, u32, u32) {
        let o = i * CELL_LEN;
        let b = &self.cells[o..o + CELL_LEN];
        (
            u64::from_le_bytes(b[0..8].try_into().unwrap()),
            u32::from_le_bytes(b[8..12].try_into().unwrap()),
            u32::from_le_bytes(b[12..16].try_into().unwrap()),
        )
    }

    /// The cell-key scheme this index was written with.
    fn key_of(&self, cx: i32, cy: i32, cz: i32) -> u64 {
        if self.version >= 3 {
            morton_cell_key(cx, cy, cz)
        } else {
            cell_key(cx, cy, cz)
        }
    }

    /// Visit every occupied cell whose coordinates lie in the inclusive
    /// box `lo..=hi`, with its record range. On a v3 index this walks the
    /// morton-sorted cell array once, jumping over out-of-box gaps with
    /// [`morton_bigmin`] — the win over per-cell probing grows with how
    /// empty the box is. Pre-v3 indexes fall back to probing each cell.
    pub fn for_each_cell_in_box(
        &self,
        lo: (i32, i32, i32),
        hi: (i32, i32, i32),
        mut f: impl FnMut(i32, i32, i32, u32, u32) -> std::ops::ControlFlow<()>,
    ) {
        use std::ops::ControlFlow;
        // Measured on routing/45 and the synthetic void galaxy. Probing
        // wins on TINY boxes (the fine phase's 27-125 cells: a walk pays
        // its entry search and bigmin restarts for nothing, 0.3-0.8x at
        // 27-150 cells) and on full-index dense boxes above ~10k cells
        // (bench-only; no production path scans those). Everything else
        // walks better: a fully dense 1,728-cell box 6x (in-box cells
        // are contiguous in the morton file, so the walk is a linear
        // scan where probing pays a binary search per cell), corridors
        // 1.4-7.5x, and mostly-empty boxes most of all -- the coarse
        // search's widened void rescans probed ~1,300 cells for nothing,
        // 70% of coarse time on thin topology, walk takes that section
        // 1.5x. The old threshold (walk only above 2048) left both the
        // widen and the dense mid-size wins on the table.
        let volume = (hi.0 - lo.0 + 1) as i64 * (hi.1 - lo.1 + 1) as i64 * (hi.2 - lo.2 + 1) as i64;
        if self.version < 3 || volume <= 216 {
            for cx in lo.0..=hi.0 {
                for cy in lo.1..=hi.1 {
                    for cz in lo.2..=hi.2 {
                        if let Some((start, count)) = self.cell_range(cx, cy, cz) {
                            if let ControlFlow::Break(()) = f(cx, cy, cz, start, count) {
                                return;
                            }
                        }
                    }
                }
            }
            return;
        }
        let zmin = morton_cell_key(lo.0, lo.1, lo.2);
        let zmax = morton_cell_key(hi.0, hi.1, hi.2);
        let cells = self.cell_count();
        // Lower bound for `key` in the sorted cell array, from `from`.
        let lower_bound = |from: usize, key: u64| -> usize {
            let (mut lo_i, mut hi_i) = (from, cells);
            while lo_i < hi_i {
                let mid = (lo_i + hi_i) / 2;
                if self.cell_entry(mid).0 < key {
                    lo_i = mid + 1;
                } else {
                    hi_i = mid;
                }
            }
            lo_i
        };
        let mut i = lower_bound(0, zmin);
        while i < cells {
            let (key, start, count) = self.cell_entry(i);
            if key > zmax {
                break;
            }
            let (cx, cy, cz) = morton_cell_of(key);
            let inside = (lo.0..=hi.0).contains(&cx)
                && (lo.1..=hi.1).contains(&cy)
                && (lo.2..=hi.2).contains(&cz);
            if inside {
                if self.cell_data_present(i) {
                    if let std::ops::ControlFlow::Break(()) = f(cx, cy, cz, start, count) {
                        return;
                    }
                }
                i += 1;
            } else {
                let Some(next) = morton_bigmin(key, zmin, zmax) else {
                    break;
                };
                i = lower_bound(i + 1, next);
            }
        }
    }

    /// Position of a cell in the sorted cell array, if occupied.
    pub(crate) fn cell_index(&self, cx: i32, cy: i32, cz: i32) -> Option<usize> {
        let key = self.key_of(cx, cy, cz);
        let (mut lo, mut hi) = (0usize, self.cell_count());
        while lo < hi {
            let mid = (lo + hi) / 2;
            let k = self.cell_entry(mid).0;
            if k == key {
                return Some(mid);
            } else if k < key {
                lo = mid + 1;
            } else {
                hi = mid;
            }
        }
        None
    }

    /// [`Galaxy::cell_index`] for the cell containing a position.
    pub fn cell_index_of_pos(&self, pos: [f32; 3]) -> Option<usize> {
        let (cx, cy, cz) = cell_of_with(pos, self.cell_ly);
        self.cell_index(cx, cy, cz)
    }

    /// The index's ALT landmark table, if `alt250.bin` was written
    /// beside it (sub-index builds write one; see [`crate::alt`]). A
    /// table sized for a different cell array is stale and ignored.
    pub fn alt(&self) -> Option<&crate::alt::AltOracle> {
        self.alt
            .get_or_init(|| {
                crate::alt::AltOracle::open(&self.dir.join(crate::alt::ALT_FILE))
                    .ok()
                    .filter(|alt| alt.leaf_count() == self.cell_count())
            })
            .as_ref()
    }

    /// Record index range for a cell by its leaf index (its position in
    /// the sorted cell array -- the numbering the cell graph and goal
    /// field use), honoring sparse-install presence like `cell_range`.
    pub fn cell_records(&self, leaf: usize) -> Option<(u32, u32)> {
        if leaf >= self.cell_count() {
            return None;
        }
        let (_, start, count) = self.cell_entry(leaf);
        self.cell_data_present(leaf).then_some((start, count))
    }

    /// Record index range for a cell, if occupied -- and, on a sparse
    /// install, only once its bytes have actually landed: a hole reads
    /// as zeros, not as records, so an absent cell is served as empty.
    pub fn cell_range(&self, cx: i32, cy: i32, cz: i32) -> Option<(u32, u32)> {
        let key = self.key_of(cx, cy, cz);
        let (mut lo, mut hi) = (0usize, self.cell_count());
        while lo < hi {
            let mid = (lo + hi) / 2;
            let (k, start, count) = self.cell_entry(mid);
            if k == key {
                return self.cell_data_present(mid).then_some((start, count));
            } else if k < key {
                lo = mid + 1;
            } else {
                hi = mid;
            }
        }
        None
    }

    /// Every system within `radius` of `pos`, with its distance. Visits at
    /// most the cells overlapping the sphere's bounding box.
    pub fn within(&self, pos: [f32; 3], radius: f32) -> Vec<(u32, f32)> {
        let mut out = Vec::new();
        self.for_each_within(pos, radius, |idx, d| out.push((idx, d)));
        out
    }

    /// Position only, straight off the map: the hot path of every search.
    #[inline]
    pub fn pos_of(&self, idx: u32) -> [f32; 3] {
        let o = HEADER_LEN + idx as usize * self.record_len;
        let b = &self.stars[o..o + 12];
        [
            f32::from_le_bytes([b[0], b[1], b[2], b[3]]),
            f32::from_le_bytes([b[4], b[5], b[6], b[7]]),
            f32::from_le_bytes([b[8], b[9], b[10], b[11]]),
        ]
    }

    /// Star class code without decoding the record.
    #[inline]
    pub fn class_code(&self, idx: u32) -> u8 {
        let packed = self.stars[HEADER_LEN + idx as usize * self.record_len + 12];
        if self.version == 1 {
            packed
        } else {
            packed & 0x0f
        }
    }

    #[inline]
    pub fn flags(&self, idx: u32) -> u8 {
        let o = HEADER_LEN + idx as usize * self.record_len;
        if self.version == 1 {
            self.stars[o + 13]
        } else {
            self.stars[o + 12] >> 4
        }
    }

    /// Supercruise distance to the scoopable companion, if the index knows
    /// it (v3 with the bucket set; the decoded value is the bucket's
    /// midpoint, within ~13 % of the true distance).
    pub fn companion_ls(&self, idx: u32) -> Option<f64> {
        if self.version < 2 {
            return None;
        }
        companion_bucket_ls(self.stars[HEADER_LEN + idx as usize * self.record_len + 13])
    }

    /// Can the ship refuel on arrival: a scoopable main star, or a scoopable
    /// companion close to the arrival point (the flag set at import).
    pub fn scoopable(&self, idx: u32) -> bool {
        let code = self.class_code(idx);
        let class = if code == crate::StarClass::Unknown.code() {
            self.class(&self.record(idx))
        } else {
            crate::StarClass::from_code(code)
        };
        class.scoopable() || self.flags(idx) & FLAG_SCOOP_NEARBY != 0
    }

    pub fn for_each_within(&self, pos: [f32; 3], radius: f32, f: impl FnMut(u32, f32)) {
        self.for_each_within_toward(pos, radius, None, f)
    }

    /// `for_each_within`, skipping whole cells that cannot hold a system
    /// closer than `max_goal` to `goal` -- a search that only wants to make
    /// progress does not need the half of a 400 ly sphere that points the
    /// wrong way.
    pub fn for_each_within_toward(
        &self,
        pos: [f32; 3],
        radius: f32,
        goal: Option<([f32; 3], f32)>,
        mut f: impl FnMut(u32, f32),
    ) {
        let cell_ly = self.cell_ly;
        let lo = cell_of_with([pos[0] - radius, pos[1] - radius, pos[2] - radius], cell_ly);
        let hi = cell_of_with([pos[0] + radius, pos[1] + radius, pos[2] + radius], cell_ly);
        let r2 = radius * radius;
        // Nearest point of a cell's box to `pos`, per axis.
        let axis_gap = |c: i32, p: f32| -> f32 {
            let lo = c as f32 * cell_ly;
            let hi = lo + cell_ly;
            if p < lo {
                lo - p
            } else if p > hi {
                p - hi
            } else {
                0.0
            }
        };
        // Cell iteration goes through the box walk: below the crossover it
        // probes in the same x,y,z order as before; a corridor- or
        // gateway-sized sphere takes the morton walk and only ever sees
        // occupied cells, so the sphere and goal prunes run per occupied
        // cell instead of per box cell.
        self.for_each_cell_in_box(lo, hi, |cx, cy, cz, start, count| {
            let (gx, gy, gz) = (
                axis_gap(cx, pos[0]),
                axis_gap(cy, pos[1]),
                axis_gap(cz, pos[2]),
            );
            if gx * gx + gy * gy + gz * gz > r2 {
                return std::ops::ControlFlow::Continue(()); // the sphere never touches this cell
            }
            if let Some((gp, max_goal)) = goal {
                // Nearest point of this cell's box to the goal.
                let nearest = |c: i32, p: f32| -> f32 {
                    let lo = c as f32 * cell_ly;
                    let hi = lo + cell_ly;
                    if p < lo {
                        lo - p
                    } else if p > hi {
                        p - hi
                    } else {
                        0.0
                    }
                };
                let (ax, ay, az) = (nearest(cx, gp[0]), nearest(cy, gp[1]), nearest(cz, gp[2]));
                if ax * ax + ay * ay + az * az > max_goal * max_goal {
                    return std::ops::ControlFlow::Continue(());
                }
            }
            let base = HEADER_LEN + start as usize * self.record_len;
            let bytes = &self.stars[base..base + count as usize * self.record_len];
            for (k, rec) in bytes.chunks_exact(self.record_len).enumerate() {
                let x = f32::from_le_bytes([rec[0], rec[1], rec[2], rec[3]]) - pos[0];
                let y = f32::from_le_bytes([rec[4], rec[5], rec[6], rec[7]]) - pos[1];
                let z = f32::from_le_bytes([rec[8], rec[9], rec[10], rec[11]]) - pos[2];
                let d2 = x * x + y * y + z * z;
                if d2 <= r2 {
                    f(start + k as u32, d2.sqrt());
                }
            }
            std::ops::ControlFlow::Continue(())
        });
    }

    /// Exact (case-insensitive) name lookup. On a sparse install the
    /// search bails with `None` the moment it would read a record whose
    /// bytes have not landed -- a hole reads as zeros, and comparing
    /// against a zeroed name would silently corrupt the binary search.
    /// The caller treats `None` as "not known locally", same as any
    /// unknown name; the fetcher is what makes the answer better.
    pub fn find(&self, name: &str) -> Option<u32> {
        let sparse = self.presence().is_some_and(|p| p.missing() > 0);
        let want = name.trim().to_lowercase();
        let n = self.byname.len() / 4;
        let (mut lo, mut hi) = (0usize, n);
        while lo < hi {
            let mid = (lo + hi) / 2;
            let idx = u32::from_le_bytes(self.byname[mid * 4..mid * 4 + 4].try_into().unwrap());
            if sparse && !self.cell_data_present(self.cell_of_record(idx)) {
                return None;
            }
            let r = self.record(idx);
            let have = self.name(&r).to_lowercase();
            match have.as_str().cmp(want.as_str()) {
                std::cmp::Ordering::Equal => return Some(idx),
                std::cmp::Ordering::Less => lo = mid + 1,
                std::cmp::Ordering::Greater => hi = mid,
            }
        }
        None
    }

    /// Names starting with `prefix`, for autocomplete. Case-insensitive.
    /// On a sparse install this stops at the first record whose bytes
    /// have not landed (see [`Galaxy::find`]) -- fewer suggestions, no
    /// garbage ones.
    pub fn complete(&self, prefix: &str, limit: usize) -> Vec<u32> {
        let sparse = self.presence().is_some_and(|p| p.missing() > 0);
        let landed = |idx: u32| !sparse || self.cell_data_present(self.cell_of_record(idx));
        let want = prefix.trim().to_lowercase();
        if want.is_empty() {
            return Vec::new();
        }
        let n = self.byname.len() / 4;
        let (mut lo, mut hi) = (0usize, n);
        while lo < hi {
            let mid = (lo + hi) / 2;
            let idx = u32::from_le_bytes(self.byname[mid * 4..mid * 4 + 4].try_into().unwrap());
            if !landed(idx) {
                return Vec::new();
            }
            let r = self.record(idx);
            if self.name(&r).to_lowercase().as_str() < want.as_str() {
                lo = mid + 1;
            } else {
                hi = mid;
            }
        }
        let mut out = Vec::new();
        let mut i = lo;
        while i < n && out.len() < limit {
            let idx = u32::from_le_bytes(self.byname[i * 4..i * 4 + 4].try_into().unwrap());
            if !landed(idx) {
                break;
            }
            let r = self.record(idx);
            if !self.name(&r).to_lowercase().starts_with(&want) {
                break;
            }
            out.push(idx);
            i += 1;
        }
        out
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::StarClass;

    #[test]
    fn records_round_trip_through_bytes() {
        let r = StarRecord {
            x: 1.5,
            y: -2.25,
            z: 3.0,
            class: 6,
            flags: 1,
            companion: 17,
            name_len: 5,
            id64: 12345,
            name_off: 99,
        };
        let mut buf = Vec::new();
        r.write_to(&mut buf);
        assert_eq!(buf.len(), RECORD_LEN);
        assert_eq!(StarRecord::read_from(&buf), r);
    }

    #[test]
    fn opens_legacy_v1_records() {
        let dir = tempfile::tempdir().unwrap();
        let mut stars = vec![0u8; HEADER_LEN];
        stars[0..4].copy_from_slice(MAGIC);
        stars[4..8].copy_from_slice(&1u32.to_le_bytes());
        stars[8..16].copy_from_slice(&1u64.to_le_bytes());
        stars[16..20].copy_from_slice(&CELL_LY.to_le_bytes());
        stars.extend_from_slice(&1.5f32.to_le_bytes());
        stars.extend_from_slice(&(-2.25f32).to_le_bytes());
        stars.extend_from_slice(&3.0f32.to_le_bytes());
        stars.push(StarClass::K.code());
        stars.push(FLAG_MAIN_STAR);
        stars.extend_from_slice(&3u16.to_le_bytes());
        stars.extend_from_slice(&12345u64.to_le_bytes());
        stars.extend_from_slice(&0u64.to_le_bytes());
        std::fs::write(dir.path().join("stars.bin"), stars).unwrap();
        std::fs::write(dir.path().join("cells.bin"), []).unwrap();
        std::fs::write(dir.path().join("names.bin"), b"Sol").unwrap();
        std::fs::write(dir.path().join("byname.bin"), 0u32.to_le_bytes()).unwrap();
        let g = Galaxy::open(dir.path()).unwrap();
        let r = g.record(0);
        assert_eq!(r.pos(), [1.5, -2.25, 3.0]);
        assert_eq!(r.id64, 12345);
        assert_eq!(g.name(&r), "Sol");
        assert_eq!(g.class(&r), StarClass::K);
        assert_eq!(g.flags(0), FLAG_MAIN_STAR);
    }

    /// The Morton key interleaves bits: spatial neighbours sort near each
    /// other, every distinct cell gets a distinct key, and the encoding is
    /// frozen (golden values) so files outlive refactors.
    #[test]
    fn morton_keys_interleave_and_stay_frozen() {
        let mut seen = std::collections::HashSet::new();
        for x in -2..=2 {
            for y in -2..=2 {
                for z in -2..=2 {
                    assert!(
                        seen.insert(morton_cell_key(x, y, z)),
                        "duplicate key at {x},{y},{z}"
                    );
                }
            }
        }
        // Locality: one step in any axis flips low-order bits only; the
        // distance to a far cell dominates every near neighbour.
        let base = morton_cell_key(10, 20, 30);
        let near = [
            morton_cell_key(11, 20, 30),
            morton_cell_key(10, 21, 30),
            morton_cell_key(10, 20, 31),
        ];
        let far = morton_cell_key(1000, 20, 30);
        for n in near {
            assert!(base.abs_diff(n) < base.abs_diff(far));
        }
        // Frozen values: a change here is a format break, not a refactor.
        // Cell (0,0,0) is biased to (2^20, 2^20, 2^20); interleaving puts
        // each axis's bit 20 at positions 62, 61, 60.
        assert_eq!(morton_cell_key(0, 0, 0), 0x7000_0000_0000_0000);
        assert_eq!(morton_cell_key(1, 0, 0) ^ morton_cell_key(0, 0, 0), 0b100);
        assert_eq!(morton_cell_key(0, 1, 0) ^ morton_cell_key(0, 0, 0), 0b010);
        assert_eq!(morton_cell_key(0, 0, 1) ^ morton_cell_key(0, 0, 0), 0b001);
    }

    /// Buckets are monotonic in distance, cover (0, 1500], and decode to
    /// within ~13 % of what was encoded. Zero means unknown.
    #[test]
    fn companion_buckets_round_trip_within_tolerance() {
        assert_eq!(companion_bucket_ls(0), None);
        assert_eq!(companion_bucket_ls(COMPANION_BUCKETS + 1), None);
        let mut previous = 0u8;
        for ls in [
            1.0, 2.0, 5.0, 12.0, 40.0, 100.0, 320.0, 700.0, 1_499.0, 1_500.0,
        ] {
            let bucket = companion_bucket(ls);
            assert!(
                (1..=COMPANION_BUCKETS).contains(&bucket),
                "{ls} ls -> bucket {bucket}"
            );
            assert!(bucket >= previous, "buckets must be monotonic in distance");
            previous = bucket;
            let decoded = companion_bucket_ls(bucket).unwrap();
            let ratio = decoded / ls;
            assert!(
                (0.85..=1.18).contains(&ratio),
                "{ls} ls decoded as {decoded:.1} ls (bucket {bucket})"
            );
        }
        assert_eq!(
            companion_bucket(0.0),
            1,
            "clamped, never zero: zero is reserved for unknown"
        );
        assert_eq!(companion_bucket(9_999.0), COMPANION_BUCKETS);
    }

    /// Decode inverts encode over the whole coordinate range.
    #[test]
    fn morton_decode_inverts_encode() {
        for (x, y, z) in [
            (0, 0, 0),
            (1, -1, 2),
            (-1_048_576, 1_048_575, 0),
            (517, -33, 9_812),
        ] {
            assert_eq!(morton_cell_of(morton_cell_key(x, y, z)), (x, y, z));
        }
    }

    /// BIGMIN agrees with brute force over every (probe, box) pair in a
    /// small grid — the property that makes range walking correct.
    #[test]
    fn bigmin_matches_brute_force_on_a_small_grid() {
        let r = -3..=3i32;
        let mut keys: Vec<(u64, (i32, i32, i32))> = Vec::new();
        for x in r.clone() {
            for y in r.clone() {
                for z in r.clone() {
                    keys.push((morton_cell_key(x, y, z), (x, y, z)));
                }
            }
        }
        keys.sort();
        let boxes = [
            ((-3, -3, -3), (3, 3, 3)),
            ((-1, 0, 1), (2, 2, 3)),
            ((0, 0, 0), (0, 0, 0)),
            ((-3, 2, -2), (-1, 3, 0)),
        ];
        for (lo, hi) in boxes {
            let zmin = morton_cell_key(lo.0, lo.1, lo.2);
            let zmax = morton_cell_key(hi.0, hi.1, hi.2);
            let in_box = |c: (i32, i32, i32)| {
                (lo.0..=hi.0).contains(&c.0)
                    && (lo.1..=hi.1).contains(&c.1)
                    && (lo.2..=hi.2).contains(&c.2)
            };
            for (probe, cell) in &keys {
                if in_box(*cell) {
                    continue; // the walk only calls bigmin on misses
                }
                let expected = keys
                    .iter()
                    .filter(|(k, c)| in_box(*c) && k > probe)
                    .map(|(k, _)| *k)
                    .min();
                assert_eq!(
                    morton_bigmin(*probe, zmin, zmax),
                    expected,
                    "bigmin diverged for probe {cell:?} over box {lo:?}..{hi:?}"
                );
            }
        }
    }

    /// The box walk visits exactly the occupied in-box cells, in file
    /// order, on both a v3 index and the v2 fallback path.
    #[test]
    fn box_walk_visits_exactly_the_occupied_cells() {
        let dir = tempfile::tempdir().unwrap();
        let mut source = String::from(
            "[
",
        );
        let coords = [
            (0, 0, 0),
            (60, 0, 0),
            (-60, 0, 0),
            (120, 60, -60),
            (300, 300, 300),
        ];
        for (i, (x, y, z)) in coords.iter().enumerate() {
            source.push_str(&format!(
                "{{\"id64\":{},\"name\":\"S{i}\",\"coords\":{{\"x\":{x}.0,\"y\":{y}.0,\"z\":{z}.0}},\"bodies\":[]}}{}
",
                i + 1,
                if i + 1 == coords.len() { "" } else { "," }
            ));
        }
        source.push(']');
        crate::import::import_reader(
            Box::new(std::io::Cursor::new(source.into_bytes())),
            dir.path(),
            &mut |_| {},
        )
        .unwrap();
        let g = Galaxy::open(dir.path()).unwrap();
        let mut seen = Vec::new();
        g.for_each_cell_in_box((-2, -2, -2), (2, 2, 2), |cx, cy, cz, start, count| {
            seen.push(((cx, cy, cz), start, count));
            std::ops::ControlFlow::Continue(())
        });
        let cells: Vec<(i32, i32, i32)> = seen.iter().map(|(c, _, _)| *c).collect();
        assert!(
            cells.contains(&(0, 0, 0))
                && cells.contains(&(1, 0, 0))
                && cells.contains(&(-2, 0, 0))
                && cells.contains(&(2, 1, -2))
        );
        assert_eq!(
            cells.len(),
            4,
            "the far cell (300,300,300)/(6,6,6) is outside the box"
        );
        let total: u32 = seen.iter().map(|(_, _, n)| *n).sum();
        assert_eq!(total, 4);
        // Agreement with per-cell probing over the same box.
        let mut probed = Vec::new();
        for cx in -2..=2 {
            for cy in -2..=2 {
                for cz in -2..=2 {
                    if let Some((start, count)) = g.cell_range(cx, cy, cz) {
                        probed.push(((cx, cy, cz), start, count));
                    }
                }
            }
        }
        let mut sorted = seen.clone();
        sorted.sort();
        probed.sort();
        assert_eq!(sorted, probed);
    }

    /// A sphere scan big enough to take the morton walk (box volume above
    /// the probe/walk crossover) finds exactly the brute-force set, and
    /// the toward-variant still drops cells that cannot approach the goal.
    /// Pins the walk delegation inside `for_each_within_toward` — the
    /// corridor dump and the gateway scan are this call at scale.
    #[test]
    fn sphere_scans_agree_with_brute_force_on_walk_sized_boxes() {
        let dir = tempfile::tempdir().unwrap();
        let mut source = String::from("[\n");
        let coords = [
            (0, 0, 0),
            (60, 0, 0),
            (-60, 0, 0),
            (120, 60, -60),
            (300, 300, 300),
        ];
        for (i, (x, y, z)) in coords.iter().enumerate() {
            source.push_str(&format!(
                "{{\"id64\":{},\"name\":\"S{i}\",\"coords\":{{\"x\":{x}.0,\"y\":{y}.0,\"z\":{z}.0}},\"bodies\":[]}}{}\n",
                i + 1,
                if i + 1 == coords.len() { "" } else { "," }
            ));
        }
        source.push(']');
        crate::import::import_reader(
            Box::new(std::io::Cursor::new(source.into_bytes())),
            dir.path(),
            &mut |_| {},
        )
        .unwrap();
        let g = Galaxy::open(dir.path()).unwrap();
        // Radius 380 at 50 ly cells: a 16^3 = 4096 cell box, above the
        // 2048-cell crossover, so the scan runs on the walked path.
        let names_within = |center: [f32; 3], radius: f32, goal: Option<([f32; 3], f32)>| {
            let mut names = Vec::new();
            g.for_each_within_toward(center, radius, goal, |idx, d| {
                let r = g.record(idx);
                let p = r.pos();
                let dx = p[0] - center[0];
                let dy = p[1] - center[1];
                let dz = p[2] - center[2];
                let brute = (dx * dx + dy * dy + dz * dz).sqrt();
                assert!(
                    (d - brute).abs() < 0.01,
                    "reported distance disagrees for {}",
                    g.name(&r)
                );
                names.push(g.name(&r).to_string());
            });
            names.sort();
            names
        };
        // S4 at (300,300,300) is 519 ly out; everything else is inside.
        assert_eq!(
            names_within([0.0, 0.0, 0.0], 380.0, None),
            ["S0", "S1", "S2", "S3"].map(String::from)
        );
        // Toward a goal at (300,0,0) with 260 ly of slack: the cell of S2
        // (-60,0,0) sits 350 ly from the goal and is skipped whole.
        assert_eq!(
            names_within([0.0, 0.0, 0.0], 380.0, Some(([300.0, 0.0, 0.0], 260.0))),
            ["S0", "S1", "S3"].map(String::from)
        );
    }

    /// A sparse (chunked) install: cells whose bytes have not landed are
    /// served as empty by every spatial query, enumerated for the
    /// fetcher, and never read as names -- and a bitmap sized for some
    /// other index is ignored entirely.
    #[test]
    fn a_sparse_install_serves_only_landed_cells() {
        let dir = tempfile::tempdir().unwrap();
        let mut source = String::from("[\n");
        let coords = [
            (0, 0, 0),
            (60, 0, 0),
            (-60, 0, 0),
            (120, 60, -60),
            (300, 300, 300),
        ];
        for (i, (x, y, z)) in coords.iter().enumerate() {
            source.push_str(&format!(
                "{{\"id64\":{},\"name\":\"S{i}\",\"coords\":{{\"x\":{x}.0,\"y\":{y}.0,\"z\":{z}.0}},\"bodies\":[]}}{}\n",
                i + 1,
                if i + 1 == coords.len() { "" } else { "," }
            ));
        }
        source.push(']');
        crate::import::import_reader(
            Box::new(std::io::Cursor::new(source.into_bytes())),
            dir.path(),
            &mut |_| {},
        )
        .unwrap();
        let g = Galaxy::open(dir.path()).unwrap();
        assert_eq!(
            g.within([0.0, 0.0, 0.0], 400.0).len(),
            4,
            "S0..S3 in range, S4 far out"
        );
        let hole = g.cell_of_record(g.find("S1").unwrap());
        let mut p = crate::presence::Presence::new_absent(g.cell_count());
        for i in 0..g.cell_count() {
            if i != hole {
                p.mark_present(i);
            }
        }
        p.write(&dir.path().join(crate::presence::PRESENT_FILE))
            .unwrap();

        // A fresh handle picks the bitmap up; the old one cached "none".
        let g = Galaxy::open(dir.path()).unwrap();
        let served: Vec<String> = g
            .within([0.0, 0.0, 0.0], 400.0)
            .into_iter()
            .map(|(idx, _)| g.name(&g.record(idx)).to_string())
            .collect();
        assert!(
            !served.contains(&"S1".to_string()),
            "the hole is served as empty: {served:?}"
        );
        assert_eq!(served.len(), 3);
        assert_eq!(
            g.cell_range(1, 0, 0),
            None,
            "S1's cell is occupied but not landed"
        );
        assert_eq!(
            g.missing_cells_in_box((-8, -8, -8), (8, 8, 8)),
            vec![(1, 0, 0)]
        );
        assert_eq!(
            g.missing_cells_in_box((2, 2, 2), (8, 8, 8)),
            vec![],
            "the hole is not in this box"
        );
        // Name search never reads the hole as bytes: S1 is unknown, and
        // anything that does resolve resolves correctly.
        assert_eq!(g.find("S1"), None);
        for name in ["S0", "S2", "S3", "S4"] {
            if let Some(idx) = g.find(name) {
                assert_eq!(g.name(&g.record(idx)), name);
            }
        }
        for idx in g.complete("S", 10) {
            assert!(g.name(&g.record(idx)).starts_with('S'));
        }

        // A bitmap for a different cell array is not this index's:
        // ignored, everything served.
        crate::presence::Presence::new_absent(g.cell_count() + 7)
            .write(&dir.path().join(crate::presence::PRESENT_FILE))
            .unwrap();
        let g = Galaxy::open(dir.path()).unwrap();
        assert!(g.presence().is_none());
        assert_eq!(g.within([0.0, 0.0, 0.0], 400.0).len(), 4);
        assert_eq!(
            g.find("S1").map(|i| g.name(&g.record(i)).to_string()),
            Some("S1".into())
        );
    }

    #[test]
    fn cell_keys_are_unique_and_ordered_per_axis() {
        assert_ne!(cell_key(0, 0, 0), cell_key(0, 0, 1));
        assert_ne!(cell_key(-1, 0, 0), cell_key(0, 0, 0));
        assert!(cell_key(0, 0, 1) > cell_key(0, 0, 0));
        assert_eq!(cell_of([64.15, -12.28, 98.34]), (1, -1, 1));
    }
}
