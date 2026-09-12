//! Streaming EBEX writer: the same bytes [`encode_snapshot`] produces,
//! written to a file one record at a time so a section of a hundred
//! million records never has to exist in memory.
//!
//! The container puts its directory (record counts, offsets, lengths,
//! CRC-32C) ahead of the data, so the writer reserves the header and
//! directory, appends each section's records and auxiliary bytes as they
//! come -- keeping the count, the length and an incremental CRC -- and
//! seeks back to fill the directory in when the last section is closed.
//! Sections must be opened in ascending id order, as the encoder sorts
//! them. [`finish`] validates the file through the same reader every
//! client uses.
//!
//! [`compress_file`] / [`decompress_file`] are the streaming twins of
//! [`compress`] / [`decompress`]: one zstd frame, hashed as it is written.

use std::fs::File;
use std::io::{BufReader, BufWriter, Read, Seek, SeekFrom, Write};
use std::path::{Path, PathBuf};

use anyhow::{ensure, Context, Result};
use sha2::{Digest, Sha256};

use crate::{
    align8, put_i64, put_u16, put_u32, put_u64, validate_snapshot, Crc32c, SnapshotHeader,
    SnapshotMetadata, CONTAINER_VERSION, DIRECTORY_ENTRY_BYTES, FLAG_FULL_BASELINE, HEADER_BYTES,
    MAGIC,
};

/// A section the writer will receive, in the order it will be opened.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct SectionPlan {
    pub id: u16,
    pub schema: u16,
    pub required: bool,
    /// Fixed record size, or 0 for variable records.
    pub record_size: u32,
}

#[derive(Default)]
struct Written {
    record_count: u64,
    record_offset: u64,
    record_bytes: u64,
    auxiliary_offset: u64,
    auxiliary_bytes: u64,
    crc: u32,
}

enum Phase {
    Records,
    Auxiliary,
}

pub struct SnapshotWriter {
    path: PathBuf,
    out: BufWriter<File>,
    plan: Vec<SectionPlan>,
    written: Vec<Written>,
    /// Bytes written so far (the file position, without seeking).
    cursor: u64,
    current: Option<(usize, Phase, Crc32c)>,
}

impl SnapshotWriter {
    /// Start an artifact at `path` (truncated) for exactly `plan`'s sections.
    pub fn create(path: &Path, header: SnapshotHeader, plan: &[SectionPlan]) -> Result<Self> {
        ensure!(!plan.is_empty(), "EBEX must contain at least one section");
        for pair in plan.windows(2) {
            ensure!(
                pair[0].id < pair[1].id,
                "EBEX sections must be planned in ascending id order"
            );
        }
        for section in plan {
            ensure!(
                section.id != 0 && section.schema != 0,
                "invalid EBEX section identity"
            );
        }
        let directory_bytes = plan
            .len()
            .checked_mul(DIRECTORY_ENTRY_BYTES)
            .context("EBEX directory size overflow")?;
        let data_start = align8(HEADER_BYTES + directory_bytes)?;
        let mut prefix = vec![0u8; data_start];
        prefix[0..8].copy_from_slice(MAGIC);
        put_u16(&mut prefix, 8, CONTAINER_VERSION);
        put_u16(&mut prefix, 10, HEADER_BYTES as u16);
        put_u32(&mut prefix, 12, FLAG_FULL_BASELINE);
        put_u64(&mut prefix, 16, header.sequence);
        put_i64(&mut prefix, 24, header.created_at);
        put_i64(&mut prefix, 32, header.watermark);
        put_u32(&mut prefix, 40, u32::try_from(plan.len())?);
        put_u16(&mut prefix, 44, DIRECTORY_ENTRY_BYTES as u16);
        put_u64(&mut prefix, 48, HEADER_BYTES as u64);
        let file = File::create(path).with_context(|| format!("creating {}", path.display()))?;
        let mut out = BufWriter::with_capacity(1 << 20, file);
        out.write_all(&prefix)?;
        Ok(SnapshotWriter {
            path: path.to_path_buf(),
            out,
            plan: plan.to_vec(),
            written: Vec::with_capacity(plan.len()),
            cursor: data_start as u64,
            current: None,
        })
    }

    /// Open the next planned section (they must be opened in plan order).
    pub fn begin_section(&mut self, id: u16) -> Result<()> {
        ensure!(
            self.current.is_none(),
            "EBEX section {id} opened while another is open"
        );
        let index = self.written.len();
        let planned = self
            .plan
            .get(index)
            .context("more EBEX sections than planned")?;
        ensure!(
            planned.id == id,
            "EBEX section {id} opened out of plan order (expected {})",
            planned.id
        );
        self.written.push(Written {
            record_offset: self.cursor,
            ..Written::default()
        });
        self.current = Some((index, Phase::Records, Crc32c::new()));
        Ok(())
    }

    /// Append one record of the open section. A fixed-size section checks
    /// the length; a variable one counts whatever it is given.
    pub fn write_record(&mut self, record: &[u8]) -> Result<()> {
        let (index, phase, crc) = self.current.as_mut().context("no EBEX section is open")?;
        ensure!(
            matches!(phase, Phase::Records),
            "EBEX record written after the auxiliary region"
        );
        let size = self.plan[*index].record_size;
        if size != 0 {
            ensure!(record.len() == size as usize, "EBEX record length mismatch");
        }
        self.out.write_all(record)?;
        crc.update(record);
        let w = &mut self.written[*index];
        w.record_count += 1;
        w.record_bytes += record.len() as u64;
        self.cursor += record.len() as u64;
        Ok(())
    }

    /// Append (part of) the open section's auxiliary region. The first call
    /// ends the record region; an empty auxiliary region is simply never
    /// started.
    pub fn write_auxiliary(&mut self, bytes: &[u8]) -> Result<()> {
        if bytes.is_empty() {
            return Ok(());
        }
        let (index, in_records) = match &self.current {
            Some((index, phase, _)) => (*index, matches!(phase, Phase::Records)),
            None => anyhow::bail!("no EBEX section is open"),
        };
        if in_records {
            let aligned = align8(usize::try_from(self.cursor)?)? as u64;
            self.pad_to(aligned)?;
            self.written[index].auxiliary_offset = self.cursor;
            if let Some((_, phase, _)) = self.current.as_mut() {
                *phase = Phase::Auxiliary;
            }
        }
        self.out.write_all(bytes)?;
        if let Some((_, _, crc)) = self.current.as_mut() {
            crc.update(bytes);
        }
        self.written[index].auxiliary_bytes += bytes.len() as u64;
        self.cursor += bytes.len() as u64;
        Ok(())
    }

    /// Close the open section: pad to 8 bytes and record its checksum.
    pub fn end_section(&mut self) -> Result<()> {
        let (index, _, crc) = self.current.take().context("no EBEX section is open")?;
        self.written[index].crc = crc.finish();
        let aligned = align8(usize::try_from(self.cursor)?)? as u64;
        self.pad_to(aligned)?;
        Ok(())
    }

    fn pad_to(&mut self, target: u64) -> Result<()> {
        if target > self.cursor {
            let zeros = [0u8; 8];
            let n = usize::try_from(target - self.cursor)?;
            self.out.write_all(&zeros[..n])?;
            self.cursor = target;
        }
        Ok(())
    }

    /// Fill the directory in, flush, and validate the file through the
    /// container reader. Returns the validated metadata.
    pub fn finish(mut self) -> Result<SnapshotMetadata> {
        ensure!(self.current.is_none(), "an EBEX section is still open");
        ensure!(
            self.written.len() == self.plan.len(),
            "not every planned EBEX section was written"
        );
        for (section, w) in self.plan.iter().zip(&self.written) {
            if section.record_size != 0 {
                let expected = w
                    .record_count
                    .checked_mul(u64::from(section.record_size))
                    .context("EBEX section size overflow")?;
                ensure!(expected == w.record_bytes, "EBEX record length mismatch");
            }
        }
        self.out.flush()?;
        let mut file = self.out.into_inner().map_err(|e| e.into_error())?;
        let mut directory = vec![0u8; self.plan.len() * DIRECTORY_ENTRY_BYTES];
        for (index, (section, w)) in self.plan.iter().zip(&self.written).enumerate() {
            let at = index * DIRECTORY_ENTRY_BYTES;
            put_u16(&mut directory, at, section.id);
            put_u16(&mut directory, at + 2, section.schema);
            put_u32(&mut directory, at + 4, u32::from(section.required));
            put_u64(&mut directory, at + 8, w.record_count);
            put_u32(&mut directory, at + 16, section.record_size);
            put_u64(&mut directory, at + 24, w.record_offset);
            put_u64(&mut directory, at + 32, w.record_bytes);
            if w.auxiliary_bytes != 0 {
                put_u64(&mut directory, at + 40, w.auxiliary_offset);
                put_u64(&mut directory, at + 48, w.auxiliary_bytes);
            }
            put_u32(&mut directory, at + 56, w.crc);
        }
        file.seek(SeekFrom::Start(HEADER_BYTES as u64))?;
        file.write_all(&directory)?;
        file.sync_all()?;
        drop(file);
        let map = map_file(&self.path)?;
        validate_snapshot(&map).with_context(|| format!("validating {}", self.path.display()))
    }
}

/// Read-only memory map of a whole file: the way to hand a multi-gigabyte
/// artifact to the `&[u8]` validators without reading it into memory.
pub fn map_file(path: &Path) -> Result<memmap2::Mmap> {
    let file = File::open(path).with_context(|| format!("opening {}", path.display()))?;
    // SAFETY: the file is written once and not modified while mapped.
    unsafe { memmap2::Mmap::map(&file) }.with_context(|| format!("mapping {}", path.display()))
}

/// Compress `source` into `target` as one zstd frame, streaming. Returns
/// the compressed byte count and its lowercase hex SHA-256.
pub fn compress_file(source: &Path, target: &Path, level: i32) -> Result<(u64, String)> {
    let input = BufReader::with_capacity(1 << 20, File::open(source)?);
    let output = File::create(target).with_context(|| format!("creating {}", target.display()))?;
    let mut sink = HashingWriter {
        inner: BufWriter::with_capacity(1 << 20, output),
        hash: Sha256::new(),
        bytes: 0,
    };
    {
        let mut encoder = zstd::stream::Encoder::new(&mut sink, level).context("starting zstd")?;
        std::io::copy(&mut { input }, &mut encoder).context("compressing EBEX")?;
        encoder.finish().context("finishing zstd")?;
    }
    sink.inner.flush()?;
    let bytes = sink.bytes;
    Ok((bytes, format!("{:x}", sink.hash.finalize())))
}

/// Decompress `source` (one zstd frame) into `target`, streaming.
pub fn decompress_file(source: &Path, target: &Path) -> Result<u64> {
    let input = BufReader::with_capacity(1 << 20, File::open(source)?);
    let mut decoder = zstd::stream::Decoder::new(input).context("starting zstd")?;
    let mut output = BufWriter::with_capacity(1 << 20, File::create(target)?);
    let bytes = std::io::copy(&mut decoder, &mut output).context("decompressing EBEX")?;
    output.flush()?;
    Ok(bytes)
}

struct HashingWriter<W: Write> {
    inner: W,
    hash: Sha256,
    bytes: u64,
}

impl<W: Write> Write for HashingWriter<W> {
    fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
        let n = self.inner.write(buf)?;
        self.hash.update(&buf[..n]);
        self.bytes += n as u64;
        Ok(n)
    }
    fn flush(&mut self) -> std::io::Result<()> {
        self.inner.flush()
    }
}

#[allow(dead_code)]
fn _read_all(path: &Path) -> Result<Vec<u8>> {
    let mut v = Vec::new();
    File::open(path)?.read_to_end(&mut v)?;
    Ok(v)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{encode_snapshot, Section};

    fn header() -> SnapshotHeader {
        SnapshotHeader {
            sequence: 7,
            created_at: 1_700_000_000,
            watermark: 1_699_999_000,
        }
    }

    fn sections() -> Vec<Section> {
        vec![
            Section {
                id: 1,
                schema: 1,
                required: true,
                record_count: 3,
                record_size: 4,
                records: (0u8..12).collect(),
                auxiliary: b"abc".to_vec(),
            },
            Section {
                id: 4,
                schema: 1,
                required: false,
                record_count: 0,
                record_size: 16,
                records: vec![],
                auxiliary: vec![],
            },
            Section {
                id: 5,
                schema: 2,
                required: true,
                record_count: 2,
                record_size: 0,
                records: b"hello world".to_vec(),
                auxiliary: b"1234567890".to_vec(),
            },
        ]
    }

    /// The streaming writer produces exactly the bytes the in-memory
    /// encoder does, record by record, auxiliary in pieces, padding and
    /// checksums included.
    #[test]
    fn streaming_writer_matches_the_encoder_byte_for_byte() {
        let expected = encode_snapshot(header(), sections()).unwrap();
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("snap.ebex");
        let plan: Vec<SectionPlan> = sections()
            .iter()
            .map(|s| SectionPlan {
                id: s.id,
                schema: s.schema,
                required: s.required,
                record_size: s.record_size,
            })
            .collect();
        let mut w = SnapshotWriter::create(&path, header(), &plan).unwrap();
        for s in sections() {
            w.begin_section(s.id).unwrap();
            if s.record_size != 0 {
                for record in s.records.chunks(s.record_size as usize) {
                    w.write_record(record).unwrap();
                }
            } else {
                // Variable records: two of them, split as the producer likes.
                w.write_record(b"hello ").unwrap();
                w.write_record(b"world").unwrap();
            }
            for piece in s.auxiliary.chunks(2) {
                w.write_auxiliary(piece).unwrap();
            }
            w.end_section().unwrap();
        }
        let meta = w.finish().unwrap();
        assert_eq!(meta.section_count, 3);
        assert_eq!(_read_all(&path).unwrap(), expected);
    }

    #[test]
    fn writer_refuses_out_of_order_and_mismatched_sections() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("snap.ebex");
        let plan = [
            SectionPlan {
                id: 2,
                schema: 1,
                required: true,
                record_size: 4,
            },
            SectionPlan {
                id: 1,
                schema: 1,
                required: true,
                record_size: 4,
            },
        ];
        assert!(SnapshotWriter::create(&path, header(), &plan).is_err());
        let plan = [SectionPlan {
            id: 1,
            schema: 1,
            required: true,
            record_size: 4,
        }];
        let mut w = SnapshotWriter::create(&path, header(), &plan).unwrap();
        assert!(w.begin_section(3).is_err(), "not the planned section");
        w.begin_section(1).unwrap();
        assert!(w.write_record(&[0u8; 3]).is_err(), "wrong record size");
        w.write_record(&[0u8; 4]).unwrap();
        w.write_auxiliary(b"x").unwrap();
        assert!(
            w.write_record(&[0u8; 4]).is_err(),
            "records after auxiliary"
        );
        w.end_section().unwrap();
        assert!(w.finish().is_ok());
    }

    #[test]
    fn file_compression_round_trips_and_hashes_the_compressed_bytes() {
        let dir = tempfile::tempdir().unwrap();
        let raw = dir.path().join("raw.bin");
        let bytes = encode_snapshot(header(), sections()).unwrap();
        std::fs::write(&raw, &bytes).unwrap();
        let zst = dir.path().join("raw.zst");
        let (n, sha) = compress_file(&raw, &zst, 9).unwrap();
        let compressed = std::fs::read(&zst).unwrap();
        assert_eq!(n, compressed.len() as u64);
        let expected: String = {
            use sha2::{Digest as _, Sha256};
            Sha256::digest(&compressed)
                .iter()
                .map(|b| format!("{b:02x}"))
                .collect()
        };
        assert_eq!(sha, expected);
        assert_eq!(crate::decompress(&compressed).unwrap(), bytes);
        let back = dir.path().join("back.bin");
        assert_eq!(decompress_file(&zst, &back).unwrap(), bytes.len() as u64);
        assert_eq!(std::fs::read(&back).unwrap(), bytes);
    }
}
