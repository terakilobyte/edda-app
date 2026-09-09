//! EDGO v1 — the daily routing-overlay wire format, and its client apply.
//!
//! The galaxy rarely changes: ~60k new systems and ~15k class updates a
//! week against a 199M-system index (ROUTING-NEXT item 47, churn measured
//! from EDDN). Shipping the full 11 GB product for that is absurd; shipping
//! chunk diffs is nearly as bad, because 75k scattered change points dirty
//! one chunk each at any chunk size. So the wire carries the changed
//! RECORDS, and the client folds them into its local EDGX v3 index with one
//! sequential merge-rebuild plus an atomic swap. The disk format does not
//! change; the exchange format is new.
//!
//! One file per published day, `routing-overlay-<to_version>.edgo`: a
//! 64-byte header, then a zstd-compressed record stream, strictly
//! ascending by (cell key, position bytes) — the same order as the index,
//! so application is a single merge-join pass.
//!
//! The header pins its base by CONTENT — the sha256 of the base's
//! `stars.bin` — never by version string, so a locally modified or
//! corrupted index refuses the overlay instead of quietly diverging
//! (refusal falls back to the full download; the fail-closed lesson of
//! ledger item 45). For that pin to hold across a CHAIN of dailies, the
//! server must produce each published version between rebases with this
//! same merge (`apply_overlays`), so the client's applied bytes equal the
//! server's published bytes and the manifest digests keep verifying.
//! `apply_overlays` is deterministic for exactly that reason.
//!
//! Scope (maintainer ruling, item 47): the index tracks main star class and the
//! scoop flags, nothing else. An update's mutable surface is exactly two
//! bytes (class, flags); renames are tombstone+add; adds may carry
//! `StarClass::Unknown` — the reconcile publishes what it knows, and a
//! later update teaches the class when a scan lands.

use crate::format::{
    cell_of_with, morton_cell_key, StarRecord, CELL_LEN, COMPANION_BUCKETS, HEADER_LEN, MAGIC,
    RECORD_LEN, VERSION,
};
use anyhow::{bail, ensure, Context, Result};
use sha2::{Digest, Sha256};
use std::collections::BTreeMap;
use std::io::{BufWriter, Read, Write};
use std::path::Path;

pub const OVERLAY_MAGIC: &[u8; 4] = b"EDGO";
pub const OVERLAY_VERSION: u32 = 1;
pub const OVERLAY_HEADER_LEN: usize = 64;

const OP_ADD: u8 = 1;
const OP_UPDATE: u8 = 2;
const OP_TOMBSTONE: u8 = 3;

/// A system the overlay introduces. `class` may be `StarClass::Unknown`'s
/// code — EDDN teaches positions before scans teach classes.
#[derive(Debug, Clone, PartialEq)]
pub struct AddRecord {
    pub id64: u64,
    pub class: u8,
    pub flags: u8,
    pub companion: u8,
    pub name: String,
}

#[derive(Debug, Clone, PartialEq)]
pub enum OverlayOp {
    Add(AddRecord),
    /// The two mutable bytes of a record, whole (not a diff mask).
    Update { class: u8, flags: u8 },
    Tombstone,
}

/// One wire record. Identity is `(cell, pos)` — positions are f32-exact
/// and never collide in practice; the apply refuses ambiguity rather than
/// guess.
#[derive(Debug, Clone, PartialEq)]
pub struct OverlayRecord {
    pub cell: u64,
    pub pos: [f32; 3],
    pub op: OverlayOp,
}

#[derive(Debug, Clone)]
pub struct Overlay {
    /// sha256 of the base version's `stars.bin`, as published.
    pub base_stars_sha256: [u8; 32],
    /// Unix seconds at write time; informational.
    pub created_at: u64,
    pub records: Vec<OverlayRecord>,
}

/// Identity as the merge orders it: cell key, then the raw little-endian
/// position bytes. Not spatial within a cell — just total and stable, which
/// is all determinism needs.
///
/// Two traps for anyone writing tests or reasoning about this order
/// (both bit the 2026-09-04 debugging session):
/// - The byte order is NOT numeric order: LE puts the mantissa's low
///   bytes first, so x=1.0 (…,80,3F) sorts ABOVE x=3.0 (…,40,40). A
///   "smaller position" is not a smaller identity.
/// - Cells come from `floor(coord / CELL_LY)`, so a negative coordinate
///   changes cell at zero: (2,-3,4) is NOT in Sol's cell — it's one cell
///   down in y.
type Identity = (u64, [u8; 12]);

fn identity(cell: u64, pos: [f32; 3]) -> Identity {
    let mut bytes = [0u8; 12];
    bytes[0..4].copy_from_slice(&pos[0].to_le_bytes());
    bytes[4..8].copy_from_slice(&pos[1].to_le_bytes());
    bytes[8..12].copy_from_slice(&pos[2].to_le_bytes());
    (cell, bytes)
}

impl Overlay {
    pub fn write(&self, path: &Path) -> Result<()> {
        let file = std::fs::File::create(path)
            .with_context(|| format!("creating {}", path.display()))?;
        self.write_to(BufWriter::new(file))
    }

    /// Encode. Records are sorted here so a writer may hand them over in
    /// any order; the READER enforces the ordering, because the wire
    /// contract is what a foreign writer must be held to.
    pub fn write_to(&self, mut w: impl Write) -> Result<()> {
        let mut records = self.records.clone();
        records.sort_by_key(|r| identity(r.cell, r.pos));
        for pair in records.windows(2) {
            ensure!(
                identity(pair[0].cell, pair[0].pos) != identity(pair[1].cell, pair[1].pos),
                "two overlay records share the identity (cell {}, pos {:?})",
                pair[0].cell,
                pair[0].pos
            );
        }
        let mut body: Vec<u8> = Vec::new();
        let mut name_bytes = 0u64;
        for record in &records {
            let op = match &record.op {
                OverlayOp::Add(_) => OP_ADD,
                OverlayOp::Update { .. } => OP_UPDATE,
                OverlayOp::Tombstone => OP_TOMBSTONE,
            };
            body.push(op);
            body.extend_from_slice(&record.cell.to_le_bytes());
            for axis in record.pos {
                body.extend_from_slice(&axis.to_le_bytes());
            }
            match &record.op {
                OverlayOp::Add(add) => {
                    ensure!(add.class <= 0x0f && add.flags <= 0x0f, "add fields exceed their nibbles");
                    ensure!(add.companion <= COMPANION_BUCKETS, "add companion bucket out of range");
                    let name = add.name.as_bytes();
                    ensure!(name.len() <= u16::MAX as usize, "add name too long");
                    body.extend_from_slice(&add.id64.to_le_bytes());
                    body.push(add.class);
                    body.push(add.flags);
                    body.push(add.companion);
                    body.extend_from_slice(&(name.len() as u16).to_le_bytes());
                    body.extend_from_slice(name);
                    name_bytes += name.len() as u64;
                }
                OverlayOp::Update { class, flags } => {
                    ensure!(*class <= 0x0f && *flags <= 0x0f, "update fields exceed their nibbles");
                    body.push(*class);
                    body.push(*flags);
                }
                OverlayOp::Tombstone => {}
            }
        }
        let mut header = Vec::with_capacity(OVERLAY_HEADER_LEN);
        header.extend_from_slice(OVERLAY_MAGIC);
        header.extend_from_slice(&OVERLAY_VERSION.to_le_bytes());
        header.extend_from_slice(&self.base_stars_sha256);
        header.extend_from_slice(&(records.len() as u32).to_le_bytes());
        ensure!(name_bytes <= u32::MAX as u64, "overlay name bytes exceed u32");
        header.extend_from_slice(&(name_bytes as u32).to_le_bytes());
        header.extend_from_slice(&self.created_at.to_le_bytes());
        header.resize(OVERLAY_HEADER_LEN, 0);
        w.write_all(&header)?;
        let mut encoder = zstd::Encoder::new(w, 0)?;
        encoder.write_all(&body)?;
        encoder.finish()?.flush()?;
        Ok(())
    }

    pub fn read(path: &Path) -> Result<Overlay> {
        let file = std::fs::File::open(path)
            .with_context(|| format!("opening {}", path.display()))?;
        Self::read_from(std::io::BufReader::new(file))
    }

    /// Decode and validate the wire contract: header sanity, exact record
    /// count, strictly ascending identities, finite positions, in-range
    /// nibbles, UTF-8 names, no trailing bytes.
    pub fn read_from(mut r: impl Read) -> Result<Overlay> {
        let mut header = [0u8; OVERLAY_HEADER_LEN];
        r.read_exact(&mut header).context("reading the overlay header")?;
        if &header[0..4] != OVERLAY_MAGIC {
            bail!("not an EDGO overlay");
        }
        let version = u32::from_le_bytes(header[4..8].try_into().unwrap());
        if version != OVERLAY_VERSION {
            bail!("EDGO version {version}, expected {OVERLAY_VERSION}");
        }
        let base_stars_sha256: [u8; 32] = header[8..40].try_into().unwrap();
        let count = u32::from_le_bytes(header[40..44].try_into().unwrap()) as usize;
        let name_bytes = u32::from_le_bytes(header[44..48].try_into().unwrap()) as u64;
        let created_at = u64::from_le_bytes(header[48..56].try_into().unwrap());
        if header[56..].iter().any(|byte| *byte != 0) {
            bail!("EDGO header has nonzero reserved bytes");
        }
        let mut body = Vec::new();
        zstd::Decoder::new(r)?
            .read_to_end(&mut body)
            .context("decompressing the overlay body")?;

        let mut records = Vec::with_capacity(count);
        let mut at = 0usize;
        let mut names_seen = 0u64;
        let mut previous: Option<Identity> = None;
        let take = |at: &mut usize, n: usize| -> Result<&[u8]> {
            let slice = body.get(*at..*at + n).context("overlay record stream is truncated")?;
            *at += n;
            Ok(slice)
        };
        for index in 0..count {
            let op = take(&mut at, 1)?[0];
            let cell = u64::from_le_bytes(take(&mut at, 8)?.try_into().unwrap());
            let raw = take(&mut at, 12)?;
            let axis = |i: usize| f32::from_le_bytes(raw[i..i + 4].try_into().unwrap());
            let pos = [axis(0), axis(4), axis(8)];
            if pos.iter().any(|v| !v.is_finite()) {
                bail!("overlay record {index} has a non-finite position");
            }
            let id = identity(cell, pos);
            if previous.is_some_and(|p| p >= id) {
                bail!("overlay records are not strictly ascending at record {index}");
            }
            previous = Some(id);
            let op = match op {
                OP_ADD => {
                    let id64 = u64::from_le_bytes(take(&mut at, 8)?.try_into().unwrap());
                    let class = take(&mut at, 1)?[0];
                    let flags = take(&mut at, 1)?[0];
                    let companion = take(&mut at, 1)?[0];
                    if class > 0x0f || flags > 0x0f || companion > COMPANION_BUCKETS {
                        bail!("overlay add {index} has out-of-range fields");
                    }
                    let name_len = u16::from_le_bytes(take(&mut at, 2)?.try_into().unwrap());
                    let name = std::str::from_utf8(take(&mut at, name_len as usize)?)
                        .with_context(|| format!("overlay add {index} name is not UTF-8"))?
                        .to_owned();
                    names_seen += u64::from(name_len);
                    OverlayOp::Add(AddRecord { id64, class, flags, companion, name })
                }
                OP_UPDATE => {
                    let class = take(&mut at, 1)?[0];
                    let flags = take(&mut at, 1)?[0];
                    if class > 0x0f || flags > 0x0f {
                        bail!("overlay update {index} has out-of-range fields");
                    }
                    OverlayOp::Update { class, flags }
                }
                OP_TOMBSTONE => OverlayOp::Tombstone,
                other => bail!("overlay record {index} has unknown op {other}"),
            };
            records.push(OverlayRecord { cell, pos, op });
        }
        if at != body.len() {
            bail!("overlay body has {} trailing bytes", body.len() - at);
        }
        if names_seen != name_bytes {
            bail!("overlay header claims {name_bytes} name bytes, stream carries {names_seen}");
        }
        Ok(Overlay { base_stars_sha256, created_at, records })
    }
}

/// sha256 of a directory's `stars.bin` — the content pin an overlay names
/// as its base.
pub fn stars_sha256(dir: &Path) -> Result<[u8; 32]> {
    let path = dir.join("stars.bin");
    let mut file = std::fs::File::open(&path)
        .with_context(|| format!("opening {}", path.display()))?;
    let mut hasher = Sha256::new();
    std::io::copy(&mut file, &mut hasher)?;
    Ok(hasher.finalize().into())
}

/// The net effect on one identity after folding a chain in order. Adds
/// (and replaces, which are removal-plus-append) remember WHICH link
/// introduced them: the server applies links sequentially, so link N's
/// appends precede link N+1's within a cell, and reproducing its bytes
/// means emitting adds in (link, identity) order — identity order alone
/// interleaves links and fails the digest gate (the live E2E refusal of
/// chain 45->49->50->51).
#[derive(Debug, Clone, PartialEq)]
enum FinalOp {
    Add { record: AddRecord, link: usize },
    Update { class: u8, flags: u8 },
    /// tombstone + add of the same identity across links (the rename
    /// path — updates can't touch names): the base record is REMOVED and
    /// the new one APPENDS at its link's position, exactly as the
    /// sequential applies would have it. Never emitted into the dead
    /// record's slot.
    Replace { record: AddRecord, link: usize },
    Tombstone,
}

/// Fold a chain of overlays (oldest first) into one net operation per
/// identity, so a month of dailies still costs ONE merge-rebuild pass.
/// Inconsistent chains (double add, update after tombstone) are refused —
/// a malformed chain falls closed to the full download.
fn coalesce(chain: &[Overlay]) -> Result<BTreeMap<Identity, FinalOp>> {
    let mut net: BTreeMap<Identity, FinalOp> = BTreeMap::new();
    for (link, overlay) in chain.iter().enumerate() {
        for record in &overlay.records {
            let id = identity(record.cell, record.pos);
            let prior = net.remove(&id);
            let folded = match (prior, &record.op) {
                (None, OverlayOp::Add(add)) => Some(FinalOp::Add { record: add.clone(), link }),
                (None, OverlayOp::Update { class, flags }) => {
                    Some(FinalOp::Update { class: *class, flags: *flags })
                }
                (None, OverlayOp::Tombstone) => Some(FinalOp::Tombstone),
                (Some(FinalOp::Add { record: mut add, link: added }), OverlayOp::Update { class, flags }) => {
                    add.class = *class;
                    add.flags = *flags;
                    // The append position belongs to the link that ADDED it.
                    Some(FinalOp::Add { record: add, link: added })
                }
                (Some(FinalOp::Add { .. }), OverlayOp::Tombstone) => None,
                (Some(FinalOp::Update { .. }), OverlayOp::Update { class, flags }) => {
                    Some(FinalOp::Update { class: *class, flags: *flags })
                }
                (Some(FinalOp::Update { .. }), OverlayOp::Tombstone) => Some(FinalOp::Tombstone),
                (Some(FinalOp::Tombstone), OverlayOp::Add(add)) => {
                    Some(FinalOp::Replace { record: add.clone(), link })
                }
                (Some(FinalOp::Replace { record: mut add, link: added }), OverlayOp::Update { class, flags }) => {
                    add.class = *class;
                    add.flags = *flags;
                    Some(FinalOp::Replace { record: add, link: added })
                }
                (Some(FinalOp::Replace { .. }), OverlayOp::Tombstone) => Some(FinalOp::Tombstone),
                (prior, op) => bail!(
                    "inconsistent overlay chain at cell {} pos {:?}: {op:?} after {prior:?}",
                    record.cell,
                    record.pos
                ),
            };
            if let Some(folded) = folded {
                net.insert(id, folded);
            }
        }
    }
    Ok(net)
}

#[derive(Debug, Default, Clone, serde::Serialize)]
pub struct ApplyStats {
    pub systems: u64,
    pub adds: u64,
    pub updates: u64,
    pub replaced: u64,
    pub tombstones: u64,
    pub cells: u64,
    pub name_bytes: u64,
}

/// Apply a chain of overlays (oldest first) to the index at `base_dir`,
/// writing a complete EDGX v3 index into `out_dir` (a staging directory;
/// the caller swaps it in atomically — `install_routing_dir`). The base is
/// never touched: a crash mid-apply leaves the old index live and a
/// partial staging directory to delete.
///
/// Deterministic by construction: base records keep their order, adds
/// land after a cell's base records in identity order, and byname's
/// tie-break matches the importer's. The same chain applied to the same
/// base always yields the same bytes — which is what lets the server
/// publish its own apply output and the manifest digests verify ours.
///
/// Refusals (all `Err`): empty chain, a sparse (chunked) install with
/// holes, a base whose `stars.bin` hash differs from the chain's pin, an
/// update or tombstone whose identity is missing, an add whose identity
/// already exists, ambiguous identities. The caller treats any of them as
/// "take the full download instead".
pub fn apply_overlays(base_dir: &Path, chain: &[Overlay], out_dir: &Path) -> Result<ApplyStats> {
    ensure!(!chain.is_empty(), "no overlays to apply");
    let base = crate::Galaxy::open(base_dir).context("opening the base index")?;
    if base.presence().is_some_and(|p| p.missing() > 0) {
        bail!("the base index is a sparse install with unfetched cells; overlays need a whole base");
    }
    let actual = stars_sha256(base_dir)?;
    if actual != chain[0].base_stars_sha256 {
        bail!("the installed index is not the overlay's base (stars.bin hash mismatch)");
    }
    let net = coalesce(chain)?;

    // The final count is known before the pass — any mismatch with what
    // the merge actually emits is a bug, checked at the end.
    let mut expected = base.count as i64;
    for op in net.values() {
        match op {
            FinalOp::Add { .. } => expected += 1,
            FinalOp::Tombstone => expected -= 1,
            FinalOp::Update { .. } | FinalOp::Replace { .. } => {}
        }
    }
    let expected = u64::try_from(expected).context("overlay tombstones exceed the base count")?;

    std::fs::create_dir_all(out_dir)?;
    let mut stars = BufWriter::with_capacity(
        1 << 20,
        std::fs::File::create(out_dir.join("stars.bin"))?,
    );
    let names = BufWriter::with_capacity(
        1 << 20,
        std::fs::File::create(out_dir.join("names.bin"))?,
    );
    let mut cells = BufWriter::new(std::fs::File::create(out_dir.join("cells.bin"))?);
    let mut header = Vec::with_capacity(HEADER_LEN);
    header.extend_from_slice(MAGIC);
    header.extend_from_slice(&VERSION.to_le_bytes());
    header.extend_from_slice(&expected.to_le_bytes());
    header.extend_from_slice(&base.cell_ly.to_le_bytes());
    header.resize(HEADER_LEN, 0);
    stars.write_all(&header)?;

    let mut stats = ApplyStats::default();
    struct Emitter {
        stars: BufWriter<std::fs::File>,
        names: BufWriter<std::fs::File>,
        buf: Vec<u8>,
        name_off: u64,
        written: u64,
    }
    impl Emitter {
        fn emit(&mut self, record: &StarRecord, name: &[u8]) -> Result<()> {
            debug_assert!(name.len() <= u16::MAX as usize);
            let out = StarRecord {
                name_off: self.name_off,
                name_len: name.len() as u16,
                ..*record
            };
            self.buf.clear();
            out.write_to(&mut self.buf);
            self.stars.write_all(&self.buf)?;
            self.names.write_all(name)?;
            self.name_off += name.len() as u64;
            self.written += 1;
            Ok(())
        }
    }
    let mut em = Emitter { stars, names, buf: Vec::with_capacity(RECORD_LEN), name_off: 0, written: 0 };
    let add_record = |cell_pos: &Identity, add: &AddRecord| -> StarRecord {
        let bytes = cell_pos.1;
        let axis = |i: usize| f32::from_le_bytes(bytes[i..i + 4].try_into().unwrap());
        StarRecord {
            x: axis(0),
            y: axis(4),
            z: axis(8),
            class: add.class,
            flags: add.flags,
            companion: add.companion,
            name_len: add.name.len() as u16,
            id64: add.id64,
            name_off: 0, // assigned at emit
        }
    };

    // Both sides are ascending by cell key: base via its cell array,
    // the net ops via BTreeMap order. One merge-join over cells.
    let mut ops = net.iter().peekable();
    let mut base_cell = 0usize;
    let base_cells = {
        // cells.bin length / entry — via the public surface only.
        let mut n = 0usize;
        while base.cell_records(n).is_some() {
            n += 1;
        }
        n
    };
    let write_cell = |key: u64,
                          count: u64,
                          start: u64,
                          cells: &mut BufWriter<std::fs::File>,
                          stats: &mut ApplyStats|
     -> Result<()> {
        if count == 0 {
            return Ok(()); // a cell emptied by tombstones vanishes
        }
        cells.write_all(&key.to_le_bytes())?;
        cells.write_all(&u32::try_from(start).context("record index overflow")?.to_le_bytes())?;
        cells.write_all(&u32::try_from(count).context("cell count overflow")?.to_le_bytes())?;
        stats.cells += 1;
        Ok(())
    };

    // Emit every whole-new cell of `ops` with key strictly below `limit`.
    macro_rules! drain_new_cells {
        ($limit:expr) => {
            while let Some((&(cell, _), _)) = ops.peek() {
                if cell >= $limit {
                    break;
                }
                let start = em.written;
                let mut appends: Vec<(usize, Identity, &AddRecord)> = Vec::new();
                while let Some((&(op_cell, _), _)) = ops.peek() {
                    if op_cell != cell {
                        break;
                    }
                    let (id, op) = ops.next().unwrap();
                    match op {
                        FinalOp::Add { record, link } => {
                            appends.push((*link, *id, record));
                            stats.adds += 1;
                        }
                        other => bail!(
                            "overlay {other:?} targets cell {cell} absent from the base index"
                        ),
                    }
                }
                // Sequential applies append link by link: (link, identity).
                appends.sort_by(|a, b| (a.0, a.1).cmp(&(b.0, b.1)));
                for (_, id, add) in &appends {
                    em.emit(&add_record(id, add), add.name.as_bytes())?;
                }
                write_cell(cell, em.written - start, start, &mut cells, &mut stats)?;
            }
        };
    }

    while base_cell < base_cells {
        let (key, first, count) = {
            // Public path: cell_records gives (start, count); the key comes
            // from the record's position, cell-derived.
            let (start, count) = base.cell_records(base_cell).unwrap();
            let pos = base.pos_of(start);
            let (cx, cy, cz) = cell_of_with(pos, base.cell_ly);
            (morton_cell_key(cx, cy, cz), start, count)
        };
        drain_new_cells!(key);

        // This base cell's pending ops, if any.
        let mut cell_ops: Vec<(Identity, FinalOp)> = Vec::new();
        while let Some((&(op_cell, _), _)) = ops.peek() {
            if op_cell != key {
                break;
            }
            let (id, op) = ops.next().unwrap();
            cell_ops.push((*id, op.clone()));
        }

        let start_written = em.written;
        let mut matched = vec![false; cell_ops.len()];
        let mut appends: Vec<(usize, Identity, AddRecord)> = Vec::new();
        for record_index in first..first + count {
            let record = base.record(record_index);
            let id = identity(key, record.pos());
            let hit = cell_ops.iter().position(|(op_id, _)| *op_id == id);
            match hit.map(|i| {
                matched[i] = true;
                &cell_ops[i].1
            }) {
                None => {
                    em.emit(&record, base.name(&record).as_bytes())?;
                }
                Some(FinalOp::Update { class, flags }) => {
                    let patched = StarRecord { class: *class, flags: *flags, ..record };
                    em.emit(&patched, base.name(&record).as_bytes())?;
                    stats.updates += 1;
                }
                Some(FinalOp::Replace { record: add, link }) => {
                    // Removal here; the new record APPENDS at its link's
                    // position, as the sequential applies would place it.
                    appends.push((*link, id, add.clone()));
                    stats.replaced += 1;
                }
                Some(FinalOp::Tombstone) => {
                    stats.tombstones += 1;
                }
                Some(FinalOp::Add { .. }) => bail!(
                    "overlay adds a system that already exists at cell {key} pos {:?}",
                    record.pos()
                ),
            }
        }
        // Unmatched ops in an occupied cell must be adds; an update or
        // tombstone with no record to land on is a base/overlay mismatch.
        for (index, (id, op)) in cell_ops.iter().enumerate() {
            if matched[index] {
                continue;
            }
            match op {
                FinalOp::Add { record: add, link } => {
                    appends.push((*link, *id, add.clone()));
                    stats.adds += 1;
                }
                other => bail!("overlay {other:?} matches no record in cell {key}"),
            }
        }
        appends.sort_by_key(|a| (a.0, a.1));
        for (_, id, add) in &appends {
            em.emit(&add_record(id, add), add.name.as_bytes())?;
        }
        write_cell(key, em.written - start_written, start_written, &mut cells, &mut stats)?;
        base_cell += 1;
    }
    drain_new_cells!(u64::MAX);

    ensure!(
        em.written == expected,
        "merge emitted {} records, expected {expected} — apply bug, refusing to install",
        em.written
    );
    stats.systems = em.written;
    stats.name_bytes = em.name_off;
    em.stars.flush()?;
    em.names.flush()?;
    cells.flush()?;
    let written = em.written;
    drop((em, cells));

    write_byname(out_dir, written as usize)?;
    Ok(stats)
}

/// Build `byname.bin` for a freshly merged index: record indices sorted by
/// lower-cased name with the importer's exact tie-break, read straight off
/// the staged files.
fn write_byname(dir: &Path, count: usize) -> Result<()> {
    use rayon::prelude::*;
    let stars_file = std::fs::File::open(dir.join("stars.bin"))?;
    let names_file = std::fs::File::open(dir.join("names.bin"))?;
    // SAFETY: staging files, written above and not modified concurrently.
    let stars = unsafe { memmap2::Mmap::map(&stars_file)? };
    let names = unsafe { memmap2::Mmap::map(&names_file)? };
    let name_of = |index: u32| -> &[u8] {
        let at = HEADER_LEN + index as usize * RECORD_LEN;
        let record = StarRecord::read_from(&stars[at..at + RECORD_LEN]);
        &names[record.name_off as usize..record.name_off as usize + record.name_len as usize]
    };
    let mut keys: Vec<u32> = (0..count as u32).collect();
    keys.par_sort_unstable_by(|a, b| {
        crate::import::lowercase_cmp(name_of(*a), name_of(*b)).then_with(|| a.cmp(b))
    });
    let mut w = BufWriter::with_capacity(1 << 20, std::fs::File::create(dir.join("byname.bin"))?);
    for index in keys {
        w.write_all(&index.to_le_bytes())?;
    }
    w.flush()?;
    Ok(())
}

/// Convenience for writers and tests: the overlay record for adding a
/// system at `pos` (cell key derived, never trusted from the caller).
pub fn add_at(pos: [f32; 3], add: AddRecord) -> OverlayRecord {
    let (cx, cy, cz) = cell_of_with(pos, crate::format::CELL_LY);
    OverlayRecord { cell: morton_cell_key(cx, cy, cz), pos, op: OverlayOp::Add(add) }
}

/// See [`add_at`].
pub fn update_at(pos: [f32; 3], class: u8, flags: u8) -> OverlayRecord {
    let (cx, cy, cz) = cell_of_with(pos, crate::format::CELL_LY);
    OverlayRecord { cell: morton_cell_key(cx, cy, cz), pos, op: OverlayOp::Update { class, flags } }
}

/// See [`add_at`].
pub fn tombstone_at(pos: [f32; 3]) -> OverlayRecord {
    let (cx, cy, cz) = cell_of_with(pos, crate::format::CELL_LY);
    OverlayRecord { cell: morton_cell_key(cx, cy, cz), pos, op: OverlayOp::Tombstone }
}

// CELL_LEN is part of the format contract the apply writes; referenced so
// a format change cannot silently diverge from this module.
const _: [(); 16] = [(); CELL_LEN];

#[cfg(test)]
mod tests {
    use super::*;
    use crate::star::StarClassCode as _;
    use crate::{Galaxy, StarClass};

    const SAMPLE: &str = r#"[
{"id64":1,"name":"Sol","coords":{"x":0,"y":0,"z":0},"bodies":[{"type":"Star","subType":"G (White-Yellow) Star","mainStar":true}]},
{"id64":2,"name":"Jackson's Lighthouse","coords":{"x":-10.0,"y":5.0,"z":20.0},"bodies":[{"type":"Star","subType":"Neutron Star","mainStar":true}]},
{"id64":3,"name":"Far Away","coords":{"x":500.0,"y":0,"z":0},"bodies":[]},
{"id64":4,"name":"Wongi","coords":{"x":64.15625,"y":-12.28125,"z":98.34375},"bodies":[{"type":"Star","subType":"K (Yellow-Orange) Star","mainStar":true}]}
]"#;

    fn base_index() -> tempfile::TempDir {
        let dir = tempfile::tempdir().unwrap();
        crate::import::import_reader(Box::new(SAMPLE.as_bytes()), dir.path(), &mut |_| {})
            .unwrap();
        dir
    }

    fn overlay_for(dir: &Path, records: Vec<OverlayRecord>) -> Overlay {
        Overlay {
            base_stars_sha256: stars_sha256(dir).unwrap(),
            created_at: 1_756_857_600,
            records,
        }
    }

    fn add(name: &str, id64: u64, class: StarClass) -> AddRecord {
        AddRecord {
            id64,
            class: class.code(),
            flags: crate::format::FLAG_MAIN_STAR,
            companion: 0,
            name: name.to_owned(),
        }
    }

    /// The wire round-trips exactly — including an add whose class is
    /// Unknown, which EDDN produces until a scan lands (PC's data note).
    #[test]
    fn the_wire_round_trips_including_unknown_class_adds() {
        let records = vec![
            add_at([12.0, 3.0, -7.5], add("New Discovery", 99, StarClass::Unknown)),
            update_at([0.0, 0.0, 0.0], StarClass::G.code(), 1),
            tombstone_at([500.0, 0.0, 0.0]),
        ];
        let overlay = Overlay {
            base_stars_sha256: [7u8; 32],
            created_at: 42,
            records,
        };
        let mut bytes = Vec::new();
        overlay.write_to(&mut bytes).unwrap();
        let back = Overlay::read_from(bytes.as_slice()).unwrap();
        assert_eq!(back.base_stars_sha256, [7u8; 32]);
        assert_eq!(back.created_at, 42);
        // write_to sorts by identity; compare as sets of records.
        assert_eq!(back.records.len(), 3);
        for record in &overlay.records {
            assert!(back.records.contains(record), "{record:?} lost in transit");
        }
    }

    #[test]
    fn the_reader_refuses_malformed_overlays() {
        let good = Overlay {
            base_stars_sha256: [0u8; 32],
            created_at: 1,
            records: vec![add_at([1.0, 2.0, 3.0], add("A", 1, StarClass::K))],
        };
        let mut bytes = Vec::new();
        good.write_to(&mut bytes).unwrap();

        let mut wrong_magic = bytes.clone();
        wrong_magic[0] = b'X';
        assert!(Overlay::read_from(wrong_magic.as_slice()).is_err());

        let mut wrong_version = bytes.clone();
        wrong_version[4] = 9;
        assert!(Overlay::read_from(wrong_version.as_slice()).is_err());

        let mut trailing = bytes.clone();
        trailing[40..44].copy_from_slice(&0u32.to_le_bytes()); // claim zero records
        assert!(Overlay::read_from(trailing.as_slice()).is_err(), "body bytes with no records to own them");

        // Duplicate identities refuse at write time too.
        let dup = Overlay {
            base_stars_sha256: [0u8; 32],
            created_at: 1,
            records: vec![
                update_at([1.0, 2.0, 3.0], 1, 1),
                tombstone_at([1.0, 2.0, 3.0]),
            ],
        };
        assert!(dup.write_to(&mut Vec::new()).is_err());
    }

    /// The heart of stage 2: adds land (existing cell and brand-new cell),
    /// updates patch exactly two bytes, tombstones remove — and the merged
    /// index passes the publication validator, so it is indistinguishable
    /// from a served build.
    #[test]
    fn an_overlay_applies_adds_updates_and_tombstones() {
        let base = base_index();
        // "Far Away" (id64 3, class Unknown) learns its class; Sol gains a
        // neighbour in its own cell; a frontier system opens a new cell;
        // Jackson's Lighthouse is tombstoned (dedup correction).
        let overlay = overlay_for(
            base.path(),
            vec![
                update_at([500.0, 0.0, 0.0], StarClass::Neutron.code(), 1),
                add_at([2.0, -3.0, 4.0], add("Sol Sibling", 500, StarClass::M)),
                add_at([4000.0, 120.0, -900.0], add("Frontier AB-C d1", 501, StarClass::Unknown)),
                tombstone_at([-10.0, 5.0, 20.0]),
            ],
        );
        let out = tempfile::tempdir().unwrap();
        let stats = apply_overlays(base.path(), &[overlay], out.path()).unwrap();
        assert_eq!(
            (stats.adds, stats.updates, stats.tombstones, stats.systems),
            (2, 1, 1, 5)
        );

        Galaxy::validate_dir(out.path()).expect("the merged index must validate like a published one");
        let g = Galaxy::open(out.path()).unwrap();
        assert_eq!(g.count, 5);
        assert_eq!(g.find("Jackson's Lighthouse"), None, "tombstoned");
        let far = g.record(g.find("Far Away").expect("survives with its class taught"));
        assert_eq!(g.class(&far), StarClass::Neutron);
        assert_eq!(far.id64, 3, "an update must not touch identity fields");
        let sib = g.record(g.find("Sol Sibling").unwrap());
        assert_eq!(g.class(&sib), StarClass::M);
        assert_eq!(sib.id64, 500);
        let frontier = g.record(g.find("Frontier AB-C d1").unwrap());
        assert_eq!(g.class(&frontier), StarClass::Unknown);
        // Spatial queries see the new records where they were put.
        let near_sol: Vec<String> = g
            .within([0.0, 0.0, 0.0], 10.0)
            .into_iter()
            .map(|(i, _)| g.name(&g.record(i)).to_string())
            .collect();
        assert!(near_sol.contains(&"Sol".to_string()) && near_sol.contains(&"Sol Sibling".to_string()));
    }

    /// A two-overlay chain coalesces into one pass, and its output is
    /// byte-identical to applying the overlays one at a time — the server
    /// publishes the sequential result, so the shortcut must not diverge.
    #[test]
    fn a_chain_coalesces_to_the_sequential_result_byte_for_byte() {
        let base = base_index();
        let day1 = overlay_for(
            base.path(),
            vec![
                add_at([2.0, -3.0, 4.0], add("Newborn", 600, StarClass::Unknown)),
                update_at([0.0, 0.0, 0.0], StarClass::G.code(), 1),
            ],
        );
        // Sequential ground truth: apply day 1, then build day 2 against it.
        let after1 = tempfile::tempdir().unwrap();
        apply_overlays(base.path(), std::slice::from_ref(&day1), after1.path()).unwrap();
        let day2 = overlay_for(
            after1.path(),
            vec![
                update_at([2.0, -3.0, 4.0], StarClass::K.code(), 1), // the newborn's scan lands
                tombstone_at([500.0, 0.0, 0.0]),
            ],
        );
        let sequential = tempfile::tempdir().unwrap();
        apply_overlays(after1.path(), std::slice::from_ref(&day2), sequential.path()).unwrap();

        let chained = tempfile::tempdir().unwrap();
        let stats = apply_overlays(base.path(), &[day1, day2], chained.path()).unwrap();
        assert_eq!(stats.systems, 4);
        for name in ["stars.bin", "cells.bin", "names.bin", "byname.bin"] {
            assert_eq!(
                std::fs::read(sequential.path().join(name)).unwrap(),
                std::fs::read(chained.path().join(name)).unwrap(),
                "{name} diverged between chained and sequential application"
            );
        }
        let g = Galaxy::open(chained.path()).unwrap();
        assert_eq!(g.class(&g.record(g.find("Newborn").unwrap())), StarClass::K);
    }

    /// Every refusal fails closed: wrong base, sparse base, ops that miss,
    /// adds that collide, empty chains. The caller falls back to the full
    /// download on any of these.
    #[test]
    fn refusals_fail_closed_to_the_full_path() {
        let base = base_index();
        let out = || tempfile::tempdir().unwrap();

        assert!(apply_overlays(base.path(), &[], out().path()).is_err(), "empty chain");

        let mut wrong_base = overlay_for(base.path(), vec![tombstone_at([0.0, 0.0, 0.0])]);
        wrong_base.base_stars_sha256 = [9u8; 32];
        let err = apply_overlays(base.path(), &[wrong_base], out().path()).unwrap_err();
        assert!(err.to_string().contains("not the overlay's base"), "{err}");

        let miss = overlay_for(base.path(), vec![update_at([77.0, 77.0, 77.0], 1, 1)]);
        assert!(apply_overlays(base.path(), &[miss], out().path()).is_err(), "update with no record");

        let collide = overlay_for(
            base.path(),
            vec![add_at([0.0, 0.0, 0.0], add("Sol Imposter", 999, StarClass::G))],
        );
        assert!(apply_overlays(base.path(), &[collide], out().path()).is_err(), "add on an existing identity");

        // A sparse install (holes not yet fetched) refuses outright.
        let g = Galaxy::open(base.path()).unwrap();
        let cells = {
            let mut n = 0;
            while g.cell_records(n).is_some() {
                n += 1;
            }
            n
        };
        drop(g);
        let mut p = crate::presence::Presence::new_absent(cells);
        for i in 1..cells {
            p.mark_present(i);
        }
        p.write(&base.path().join(crate::presence::PRESENT_FILE)).unwrap();
        let sparse = overlay_for(base.path(), vec![tombstone_at([500.0, 0.0, 0.0])]);
        let err = apply_overlays(base.path(), &[sparse], out().path()).unwrap_err();
        assert!(err.to_string().contains("sparse"), "{err}");
    }

    /// The live E2E refusal of 2026-09-03 (chain 45->49->50->51): when two
    /// DIFFERENT links touch one cell, the server's sequential applies
    /// append link N's adds before link N+1's — base order, then adds by
    /// (link, identity). A coalesce that orders all adds by identity alone
    /// interleaves them and the digest gate refuses the merge. Two shapes,
    /// both must be byte-identical to sequential application:
    /// (a) link 2 adds a SMALLER identity than link 1's add in the same
    /// cell; (b) a cross-link replace (tombstone then re-add) must land at
    /// the END of the cell like a fresh append, not in the dead record's
    /// slot.
    #[test]
    fn multi_link_chains_reproduce_the_servers_sequential_bytes() {
        let base = base_index();
        // (b)'s prerequisite: a cell with two base records, tombstone the
        // one that is NOT last in file order so a trailing record exists.
        let g = Galaxy::open(base.path()).unwrap();
        let sol = g.record(g.find("Sol").unwrap());
        drop(g);

        // Day 1: add at pos whose LE-byte identity is HIGH (1.0 -> [0,0,0x80,0x3f]),
        // and tombstone Sol (first record of its cell in the tiny fixture).
        let day1 = overlay_for(
            base.path(),
            vec![
                add_at([1.0, 0.0, 0.0], add("High Identity", 800, StarClass::K)),
                tombstone_at(sol.pos()),
            ],
        );
        let after1 = tempfile::tempdir().unwrap();
        apply_overlays(base.path(), std::slice::from_ref(&day1), after1.path()).unwrap();

        // Day 2: add at pos with LOW identity (2.0 -> [0,0,0,0x40]) in the
        // SAME cell, and re-add Sol's identity (the cross-link replace).
        let day2 = overlay_for(
            after1.path(),
            vec![
                add_at([2.0, 0.0, 0.0], add("Low Identity", 801, StarClass::M)),
                add_at(sol.pos(), add("Sol Reborn", 802, StarClass::G)),
            ],
        );
        let sequential = tempfile::tempdir().unwrap();
        apply_overlays(after1.path(), std::slice::from_ref(&day2), sequential.path()).unwrap();

        let chained = tempfile::tempdir().unwrap();
        apply_overlays(base.path(), &[day1, day2], chained.path()).unwrap();
        for name in ["stars.bin", "cells.bin", "names.bin", "byname.bin"] {
            assert_eq!(
                std::fs::read(sequential.path().join(name)).unwrap(),
                std::fs::read(chained.path().join(name)).unwrap(),
                "{name}: the coalesced chain must reproduce sequential application byte for byte"
            );
        }
        Galaxy::validate_dir(chained.path()).unwrap();
    }

    /// Applying the same chain twice yields identical bytes — the property
    /// the whole digest story leans on (server publishes its own apply
    /// output; ours must match it).
    #[test]
    fn the_apply_is_byte_deterministic() {
        let base = base_index();
        let overlay = overlay_for(
            base.path(),
            vec![
                add_at([2.0, -3.0, 4.0], add("Det A", 700, StarClass::M)),
                add_at([1.0, -3.0, 4.0], add("Det B", 701, StarClass::K)),
                add_at([4000.0, 120.0, -900.0], add("Det C", 702, StarClass::Unknown)),
                update_at([64.15625, -12.28125, 98.34375], StarClass::K.code(), 1),
                tombstone_at([-10.0, 5.0, 20.0]),
            ],
        );
        let a = tempfile::tempdir().unwrap();
        let b = tempfile::tempdir().unwrap();
        apply_overlays(base.path(), std::slice::from_ref(&overlay), a.path()).unwrap();
        apply_overlays(base.path(), &[overlay], b.path()).unwrap();
        for name in ["stars.bin", "cells.bin", "names.bin", "byname.bin"] {
            assert_eq!(
                std::fs::read(a.path().join(name)).unwrap(),
                std::fs::read(b.path().join(name)).unwrap(),
                "{name} is not deterministic"
            );
        }
        // The rename path: tombstone + add at the same identity replaces
        // the record whole, in one chain.
        let rename = overlay_for(
            base.path(),
            vec![tombstone_at([0.0, 0.0, 0.0])],
        );
        let rename2 = Overlay {
            base_stars_sha256: rename.base_stars_sha256,
            created_at: 2,
            records: vec![add_at([0.0, 0.0, 0.0], add("Sol (Renamed)", 1, StarClass::G))],
        };
        let out = tempfile::tempdir().unwrap();
        let stats = apply_overlays(base.path(), &[rename, rename2], out.path()).unwrap();
        assert_eq!(stats.replaced, 1);
        let g = Galaxy::open(out.path()).unwrap();
        assert_eq!(g.find("Sol"), None);
        assert!(g.find("Sol (Renamed)").is_some());
        Galaxy::validate_dir(out.path()).unwrap();
    }
}
