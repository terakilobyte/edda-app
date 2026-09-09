//! Stream a Spansh galaxy dump into the star index.
//!
//! The dump is a JSON array with one system per line (`[`, then `{...},`
//! lines, then `]`). Each system is deserialized into a tiny struct -- id,
//! name, coordinates, and just enough of each body to find the main star's
//! class -- and everything else is skipped by serde. Nothing about the
//! JSON is retained.
//!
//! Memory: all records (32 B each) plus all names are held while sorting
//! by grid cell, so the full galaxy (~150M systems) needs ~8 GB of RAM.
//! The populated dump (153k systems) needs nothing worth mentioning and is
//! the way to test the pipeline before committing hours to the full one.

use crate::format::{
    cell_of, cell_of_with, companion_bucket, morton_cell_key, StarRecord, CELL_LEN, HEADER_LEN,
    MAGIC, RECORD_LEN, VERSION,
};
use crate::star::{StarClass, StarClassCode as _};
use anyhow::{Context, Result};
use rayon::prelude::*;
use serde::Deserialize;
use std::cmp::Ordering;
use std::io::{BufRead, BufReader, BufWriter, Read, Write};
use std::path::Path;

#[derive(Deserialize)]
struct Coords {
    x: f64,
    y: f64,
    z: f64,
}

#[derive(Deserialize)]
struct Body<'a> {
    #[serde(rename = "type", default)]
    kind: Option<&'a str>,
    #[serde(rename = "subType", default)]
    sub_type: Option<&'a str>,
    #[serde(rename = "mainStar", default)]
    main_star: Option<bool>,
    #[serde(rename = "distanceToArrival", default)]
    distance_to_arrival: Option<f64>,
}

#[derive(Deserialize)]
struct System<'a> {
    id64: u64,
    name: &'a str,
    coords: Coords,
    #[serde(default, borrow)]
    bodies: Vec<Body<'a>>,
}

/// Progress: `(systems_read, bytes_read)`.
pub type Progress<'a> = &'a mut dyn FnMut(&ImportStats);

#[derive(Debug, Default, Clone, Serialize)]
pub struct ImportStats {
    #[serde(default)]
    pub phase: String,
    /// Systems with a scoopable companion star near the arrival point.
    #[serde(default)]
    pub scoop_companions: usize,
    pub systems: u64,
    pub skipped_lines: u64,
    pub with_main_star: u64,
    pub neutron: u64,
    pub white_dwarf: u64,
    pub cells: u64,
    pub bytes_in: u64,
    pub lines: u64,
    pub longest_line: u64,
}

use serde::Serialize;

struct Pending {
    rec: StarRecord,
    cell: u64,
}

struct ParsedSystem {
    id64: u64,
    name: String,
    pos: [f32; 3],
    class: StarClass,
    has_main: bool,
    /// Distance to the nearest scoopable companion star within
    /// [`crate::format::SCOOP_COMPANION_LS`], if any.
    scoop_ls: Option<f64>,
}

enum ParsedLine {
    Empty,
    Invalid,
    System(ParsedSystem),
}

fn parse_system_line(line: &str) -> ParsedLine {
    let t = line.trim();
    let t = t.strip_suffix(',').unwrap_or(t);
    if t.is_empty() || t == "[" || t == "]" {
        return ParsedLine::Empty;
    }
    let sys: System = match serde_json::from_str(t) {
        Ok(s) => s,
        Err(_) => return ParsedLine::Invalid,
    };
    let main = sys
        .bodies
        .iter()
        .find(|b| b.main_star == Some(true) && b.kind == Some("Star"))
        .or_else(|| {
            sys.bodies
                .iter()
                .find(|b| b.kind == Some("Star"))
        });
    let class = main
        .and_then(|b| b.sub_type)
        .map(StarClass::from_subtype)
        .unwrap_or(StarClass::Unknown);
    let scoop_ls = sys
        .bodies
        .iter()
        .filter(|b| {
            b.kind == Some("Star")
                && b.main_star != Some(true)
                && b.sub_type
                    .map(StarClass::from_subtype)
                    .is_some_and(|c| c.scoopable())
        })
        .filter_map(|b| b.distance_to_arrival)
        .filter(|ls| *ls <= crate::format::SCOOP_COMPANION_LS)
        .min_by(|a, b| a.total_cmp(b));
    ParsedLine::System(ParsedSystem {
        id64: sys.id64,
        name: sys.name.to_owned(),
        pos: [
            sys.coords.x as f32,
            sys.coords.y as f32,
            sys.coords.z as f32,
        ],
        class,
        has_main: main.is_some(),
        scoop_ls,
    })
}

/// Compare names exactly as `str::to_lowercase()` does without allocating in
/// the overwhelmingly common ASCII case. Spansh system names are normally
/// ASCII; the Unicode fallback preserves the ordering expected by
/// `Galaxy::find` and `Galaxy::complete` for unusual names too.
pub(crate) fn lowercase_cmp(a: &[u8], b: &[u8]) -> Ordering {
    if a.is_ascii() && b.is_ascii() {
        return a
            .iter()
            .map(u8::to_ascii_lowercase)
            .cmp(b.iter().map(u8::to_ascii_lowercase));
    }
    String::from_utf8_lossy(a)
        .to_lowercase()
        .cmp(&String::from_utf8_lossy(b).to_lowercase())
}

fn pending_name<'a>(pending: &Pending, names: &'a [u8]) -> &'a [u8] {
    let start = pending.rec.name_off as usize;
    &names[start..start + pending.rec.name_len as usize]
}

/// Read a `.json.gz` (or plain `.json`) dump and write the index into `out_dir`.
pub fn import(path: &Path, out_dir: &Path, progress: Progress) -> Result<ImportStats> {
    let file = std::fs::File::open(path).with_context(|| format!("opening {}", path.display()))?;
    let reader: Box<dyn Read> = if path.extension().is_some_and(|e| e == "gz") {
        Box::new(flate2::read::GzDecoder::new(file))
    } else {
        Box::new(file)
    };
    import_reader(reader, out_dir, progress)
}

/// Same, from any stream -- e.g. `curl ... | gunzip` piped in, so the 116 GB
/// archive never has to touch the disk.
pub fn import_reader(
    reader: Box<dyn Read>,
    out_dir: &Path,
    progress: Progress,
) -> Result<ImportStats> {
    std::fs::create_dir_all(out_dir)?;
    let mut lines = BufReader::with_capacity(1 << 20, reader);
    let mut stats = ImportStats::default();
    stats.phase = "reading and parsing".into();

    let mut pending: Vec<Pending> = Vec::new();
    let mut names: Vec<u8> = Vec::new();
    // Keep enough work queued to occupy the parser pool without allowing a
    // fast download to turn into unbounded raw-JSON memory. Indexed parallel
    // iteration preserves source order, so record/name offsets remain
    // deterministic even though parsing happens concurrently.
    const BATCH_BYTES: usize = 32 << 20;
    const BATCH_LINES: usize = 4096;
    let mut batch: Vec<String> = Vec::with_capacity(BATCH_LINES);
    let mut last_report = std::time::Instant::now();

    loop {
        batch.clear();
        let mut batch_bytes = 0usize;
        while batch.len() < BATCH_LINES && batch_bytes < BATCH_BYTES {
            let mut line = String::new();
            let n = lines.read_line(&mut line)?;
            if n == 0 {
                break;
            }
            batch_bytes += n;
            stats.bytes_in += n as u64;
            stats.lines += 1;
            stats.longest_line = stats.longest_line.max(n as u64);
            batch.push(line);
        }
        if batch.is_empty() {
            break;
        }

        let parsed: Vec<ParsedLine> = batch
            .par_iter()
            .map(|line| parse_system_line(line))
            .collect();
        for item in parsed {
            let sys = match item {
                ParsedLine::Empty => continue,
                ParsedLine::Invalid => {
                    stats.skipped_lines += 1;
                    continue;
                }
                ParsedLine::System(sys) => sys,
            };
            if sys.has_main {
                stats.with_main_star += 1;
            }
            match sys.class {
                StarClass::Neutron => stats.neutron += 1,
                StarClass::WhiteDwarf => stats.white_dwarf += 1,
                _ => {}
            }
            if sys.scoop_ls.is_some() {
                stats.scoop_companions += 1;
            }
            let (cx, cy, cz) = cell_of(sys.pos);
            let name_off = names.len() as u64;
            let name_bytes = sys.name.as_bytes();
            let name_len = name_bytes.len().min(u16::MAX as usize) as u16;
            names.extend_from_slice(&name_bytes[..name_len as usize]);
            pending.push(Pending {
                rec: StarRecord {
                    x: sys.pos[0],
                    y: sys.pos[1],
                    z: sys.pos[2],
                    class: sys.class.code(),
                    flags: (sys.has_main as u8)
                        | if sys.scoop_ls.is_some() {
                            crate::format::FLAG_SCOOP_NEARBY
                        } else {
                            0
                        },
                    companion: sys.scoop_ls.map_or(0, companion_bucket),
                    name_len,
                    id64: sys.id64,
                    name_off,
                },
                cell: morton_cell_key(cx, cy, cz),
            });
            stats.systems += 1;
        }
        if last_report.elapsed() >= std::time::Duration::from_millis(250) {
            progress(&stats);
            last_report = std::time::Instant::now();
        }
    }
    stats.phase = "sorting spatial index".into();
    progress(&stats);
    write_index(out_dir, pending, names, &mut stats, Some(progress))?;
    stats.phase = "complete".into();
    progress(&stats);
    Ok(stats)
}

/// Write the four index files from in-memory records.
fn write_index(
    out_dir: &Path,
    pending: Vec<Pending>,
    names: Vec<u8>,
    stats: &mut ImportStats,
    progress: Option<Progress<'_>>,
) -> Result<()> {
    write_index_cells(
        out_dir,
        pending,
        names,
        stats,
        crate::format::CELL_LY,
        progress,
    )
}

fn write_index_cells(
    out_dir: &Path,
    mut pending: Vec<Pending>,
    names: Vec<u8>,
    stats: &mut ImportStats,
    cell_ly: f32,
    mut progress: Option<Progress<'_>>,
) -> Result<()> {
    // Sort by cell so each cell is one contiguous run.
    pending.par_sort_unstable_by_key(|p| p.cell);
    stats.phase = "writing spatial index".into();
    if let Some(p) = progress.as_deref_mut() {
        p(stats);
    }

    // v3's ordering guarantee: name offsets are assigned after the sort,
    // so names sit in record order and a cell's names are one contiguous
    // span of names.bin. Costs one extra copy of the name bytes at build
    // time (~4 GB peak at full-galaxy scale, on top of the ~12 GB the
    // records and old names already hold).
    let names = {
        let mut reordered = Vec::with_capacity(names.len());
        for p in &mut pending {
            let start = p.rec.name_off as usize;
            let bytes = &names[start..start + p.rec.name_len as usize];
            p.rec.name_off = reordered.len() as u64;
            reordered.extend_from_slice(bytes);
        }
        reordered
    };

    // stars.bin
    {
        let mut w =
            BufWriter::with_capacity(1 << 20, std::fs::File::create(out_dir.join("stars.bin"))?);
        let mut header = Vec::with_capacity(HEADER_LEN);
        header.extend_from_slice(MAGIC);
        header.extend_from_slice(&VERSION.to_le_bytes());
        header.extend_from_slice(&(pending.len() as u64).to_le_bytes());
        header.extend_from_slice(&cell_ly.to_le_bytes());
        header.resize(HEADER_LEN, 0);
        w.write_all(&header)?;
        let mut buf = Vec::with_capacity(RECORD_LEN * 1024);
        for p in &pending {
            p.rec.write_to(&mut buf);
            if buf.len() >= RECORD_LEN * 1024 {
                w.write_all(&buf)?;
                buf.clear();
            }
        }
        w.write_all(&buf)?;
        w.flush()?;
    }

    // cells.bin
    {
        let mut w = BufWriter::new(std::fs::File::create(out_dir.join("cells.bin"))?);
        let mut i = 0usize;
        let mut buf = Vec::with_capacity(CELL_LEN);
        while i < pending.len() {
            let key = pending[i].cell;
            let start = i;
            while i < pending.len() && pending[i].cell == key {
                i += 1;
            }
            buf.clear();
            buf.extend_from_slice(&key.to_le_bytes());
            buf.extend_from_slice(&(start as u32).to_le_bytes());
            buf.extend_from_slice(&((i - start) as u32).to_le_bytes());
            w.write_all(&buf)?;
            stats.cells += 1;
        }
        w.flush()?;
    }

    // names.bin
    std::fs::write(out_dir.join("names.bin"), &names)?;

    // byname.bin: record indices sorted by lower-cased name. Sort the compact
    // indices and compare names in place; materialising a lowercase String
    // per system took roughly 8-10 GB at full-galaxy scale.
    {
        stats.phase = "sorting name index".into();
        if let Some(p) = progress.as_deref_mut() {
            p(stats);
        }
        let mut keys: Vec<u32> = (0..pending.len() as u32).collect();
        keys.par_sort_unstable_by(|a, b| {
            lowercase_cmp(
                pending_name(&pending[*a as usize], &names),
                pending_name(&pending[*b as usize], &names),
            )
            // Match the old `(lowercase_name, record_index)` tuple ordering
            // when names differ only by case or are exact duplicates.
            .then_with(|| a.cmp(b))
        });
        stats.phase = "writing name index".into();
        if let Some(p) = progress {
            p(stats);
        }
        let mut w =
            BufWriter::with_capacity(1 << 20, std::fs::File::create(out_dir.join("byname.bin"))?);
        for idx in keys {
            w.write_all(&idx.to_le_bytes())?;
        }
        w.flush()?;
    }

    Ok(())
}

/// Write a filtered copy of an index -- e.g. neutron stars only, so the
/// long-range planner scans 1.8 % of the records instead of all of them.
/// Records keep their id64 and name, so a hit resolves back into the full
/// index with `Galaxy::find`.
pub fn subset(
    g: &crate::Galaxy,
    out_dir: &Path,
    keep: impl Fn(&StarRecord) -> bool,
) -> Result<ImportStats> {
    subset_cells(g, out_dir, crate::format::CELL_LY, keep)
}

/// `subset` with its own grid size: a sparse sub-index (neutrons are 1.8 %
/// of systems) wants cells sized to its queries, not the full index's.
pub fn subset_cells(
    g: &crate::Galaxy,
    out_dir: &Path,
    cell_ly: f32,
    keep: impl Fn(&StarRecord) -> bool,
) -> Result<ImportStats> {
    subset_cells_cancellable(g, out_dir, cell_ly, keep, &|| false)
}

/// The error a cancelled [`subset_cells_cancellable`] returns (through
/// `anyhow`; downcast to tell it from a real failure). Nothing has been
/// written when it is returned, but `out_dir` exists.
#[derive(Debug)]
pub struct SubsetCancelled;

impl std::fmt::Display for SubsetCancelled {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "sub-index build cancelled")
    }
}

impl std::error::Error for SubsetCancelled {}

/// [`subset_cells`] that polls `cancelled` as it scans (every 2^20
/// records: a scan over the full galaxy checks a few hundred times) and
/// stops with [`SubsetCancelled`] before writing anything. A build over
/// 199 M stars takes a minute; a Stop must not wait for it.
pub fn subset_cells_cancellable(
    g: &crate::Galaxy,
    out_dir: &Path,
    cell_ly: f32,
    keep: impl Fn(&StarRecord) -> bool,
    cancelled: &(dyn Fn() -> bool + Sync),
) -> Result<ImportStats> {
    std::fs::create_dir_all(out_dir)?;
    let mut pending: Vec<Pending> = Vec::new();
    let mut names: Vec<u8> = Vec::new();
    let mut stats = ImportStats::default();
    for idx in 0..g.count as u32 {
        if idx & 0xf_ffff == 0 && cancelled() {
            return Err(SubsetCancelled.into());
        }
        let rec = g.record(idx);
        if !keep(&rec) {
            continue;
        }
        let name = g.name(&rec).as_bytes();
        let name_off = names.len() as u64;
        names.extend_from_slice(name);
        let (cx, cy, cz) = cell_of_with(rec.pos(), cell_ly);
        pending.push(Pending {
            rec: StarRecord { name_off, ..rec },
            cell: morton_cell_key(cx, cy, cz),
        });
        stats.systems += 1;
    }
    if cancelled() {
        return Err(SubsetCancelled.into());
    }
    write_index_cells(out_dir, pending, names, &mut stats, cell_ly, None)?;
    // The prefix-aggregate oracle rides with the sub-index: rebuilt from
    // the freshly written cells, swapped and deleted with the directory.
    // Best effort -- a sub-index without agg250.bin still routes. The
    // ALT landmark table is NOT built here: it measured null on every
    // canonical route (the real highway has no detours big enough to
    // out-bound the straight line -- ROUTING-NEXT 5.2 ledger), so the
    // 8 s / 16 MB stay opt-in via examples/sidecars.rs; the planner uses
    // alt250.bin automatically whenever it exists.
    if let Ok(sub) = crate::Galaxy::open(out_dir) {
        if let Ok(aggregate) = crate::agg::build(&sub) {
            let _ = aggregate.write(&out_dir.join(crate::agg::AGG_FILE));
        }
    }
    Ok(stats)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::Galaxy;
    use sha2::{Digest, Sha256};

    fn fixture_hex(source: &str) -> Vec<u8> {
        source
            .split_whitespace()
            .flat_map(|word| word.as_bytes().chunks_exact(2))
            .map(|pair| {
                let digit = |byte: u8| match byte {
                    b'0'..=b'9' => byte - b'0',
                    b'a'..=b'f' => byte - b'a' + 10,
                    b'A'..=b'F' => byte - b'A' + 10,
                    _ => panic!("invalid fixture hex digit"),
                };
                digit(pair[0]) << 4 | digit(pair[1])
            })
            .collect()
    }

    fn golden_source() -> &'static [u8] {
        br#"[
{"id64":10477373803,"name":"Sol","coords":{"x":0.0,"y":0.0,"z":0.0},"bodies":[{"type":"Star","subType":"G (White-Yellow) Star","mainStar":true},{"type":"Star","subType":"K (Yellow-Orange) Star","mainStar":false,"distanceToArrival":300.0}]},
{"id64":2,"name":"Alpha","coords":{"x":60.0,"y":0.0,"z":0.0},"bodies":[{"type":"Star","subType":"K (Yellow-Orange) Star","mainStar":true}]},
{"id64":3,"name":"beta","coords":{"x":-60.0,"y":0.0,"z":0.0},"bodies":[{"type":"Star","subType":"Neutron Star","mainStar":true}]}
]"#
    }

    const SAMPLE: &str = r#"[
{"id64":1,"name":"Sol","coords":{"x":0,"y":0,"z":0},"bodies":[{"type":"Star","subType":"G (White-Yellow) Star","mainStar":true},{"type":"Planet","subType":"Rocky body"}]},
{"id64":2,"name":"Jackson's Lighthouse","coords":{"x":-10.0,"y":5.0,"z":20.0},"bodies":[{"type":"Star","subType":"Neutron Star","mainStar":true}]},
{"id64":3,"name":"Far Away","coords":{"x":500.0,"y":0,"z":0},"bodies":[]},
{"id64":4,"name":"Wongi","coords":{"x":64.15625,"y":-12.28125,"z":98.34375},"bodies":[{"type":"Star","subType":"K (Yellow-Orange) Star","mainStar":true}]}
]"#;

    /// A cancelled sub-index build writes nothing and says so; the same
    /// build uncancelled writes the index.
    #[test]
    fn a_cancelled_subset_build_writes_nothing() {
        let dir = tempfile::tempdir().unwrap();
        import_reader(Box::new(SAMPLE.as_bytes()), dir.path(), &mut |_| {}).unwrap();
        let g = crate::Galaxy::open(dir.path()).unwrap();
        let out = dir.path().join("sub");
        let err = subset_cells_cancellable(&g, &out, 100.0, |_| true, &|| true).unwrap_err();
        assert!(err.downcast_ref::<SubsetCancelled>().is_some(), "{err}");
        assert!(!crate::Galaxy::exists(&out), "a cancelled build must not leave an index");
        let stats = subset_cells_cancellable(&g, &out, 100.0, |_| true, &|| false).unwrap();
        assert_eq!(stats.systems as usize, g.count as usize);
        assert!(crate::Galaxy::exists(&out));
    }

    #[test]
    fn imports_sorts_and_queries() {
        let dir = tempfile::tempdir().unwrap();
        let mut n = 0;
        let stats = import_reader(Box::new(SAMPLE.as_bytes()), dir.path(), &mut |s| {
            n = s.systems
        })
        .unwrap();
        assert_eq!(
            (stats.systems, stats.neutron, stats.with_main_star),
            (4, 1, 3)
        );
        assert_eq!(n, 4);

        let g = Galaxy::open(dir.path()).unwrap();
        assert_eq!(g.count, 4);
        let sol = g.find("sol").expect("case-insensitive name lookup");
        let r = g.record(sol);
        assert_eq!(g.name(&r), "Sol");
        assert_eq!(g.class(&r), StarClass::G);

        let near: Vec<String> = g
            .within([0.0, 0.0, 0.0], 30.0)
            .into_iter()
            .map(|(i, _)| g.name(&g.record(i)).to_string())
            .collect();
        assert!(
            near.contains(&"Sol".to_string()) && near.contains(&"Jackson's Lighthouse".to_string())
        );
        assert!(!near.contains(&"Far Away".to_string()));

        let jl = g.record(g.find("Jackson's Lighthouse").unwrap());
        assert_eq!(g.class(&jl), StarClass::Neutron);
        assert_eq!(
            g.class(&g.record(g.find("Far Away").unwrap())),
            StarClass::Unknown
        );

        let c = g.complete("wo", 5);
        assert_eq!(c.len(), 1);
        assert_eq!(g.name(&g.record(c[0])), "Wongi");
    }

    #[test]
    fn allocation_free_name_order_matches_lowercase_strings() {
        let names: &[&str] = &[
            "Sol",
            "sOL",
            "10 G. Canis Majoris",
            "Ägir",
            "Åland",
            "İstanbul",
            "Straße",
            "Σείριος",
        ];
        for a in names {
            for b in names {
                assert_eq!(
                    lowercase_cmp(a.as_bytes(), b.as_bytes()),
                    a.to_lowercase().cmp(&b.to_lowercase()),
                    "ordering differed for {a:?} and {b:?}",
                );
            }
        }
    }

    fn synthetic_dump(count: usize) -> Vec<u8> {
        let mut out = Vec::with_capacity(count * 1_500);
        out.extend_from_slice(b"[\n");
        for i in 0..count {
            let comma = if i + 1 == count { "" } else { "," };
            writeln!(out, r#"{{"id64":{},"name":"Synthetic Sector {:06} AB-C d{}-{}","coords":{{"x":{},"y":{},"z":{}}},"bodies":[{{"type":"Star","subType":"{}","mainStar":true,"distanceToArrival":0}},{{"type":"Star","subType":"K (Yellow-Orange) Star","mainStar":false,"distanceToArrival":850}},{{"type":"Planet","subType":"Rocky body","distanceToArrival":1200}},{{"type":"Planet","subType":"High metal content world","distanceToArrival":4200}},{{"type":"Planet","subType":"Icy body","distanceToArrival":8200}},{{"type":"Planet","subType":"Gas giant with water based life","distanceToArrival":15000}}]}}{}"#,
                10_000_000_000u64 + i as u64, i % 10_000, i % 13, i % 91,
                (i as f64 % 10_000.0) - 5_000.0, (i as f64 % 700.0) - 350.0,
                (i.wrapping_mul(17) as f64 % 10_000.0) - 5_000.0,
                if i % 50 == 0 { "Neutron Star" } else { "G (White-Yellow) Star" }, comma).unwrap();
        }
        out.extend_from_slice(b"]\n");
        out
    }

    #[test]
    fn parallel_import_is_byte_deterministic() {
        let dump = synthetic_dump(2_000);
        let a = tempfile::tempdir().unwrap();
        let b = tempfile::tempdir().unwrap();
        let sa = import_reader(
            Box::new(std::io::Cursor::new(dump.clone())),
            a.path(),
            &mut |_| {},
        )
        .unwrap();
        let sb =
            import_reader(Box::new(std::io::Cursor::new(dump)), b.path(), &mut |_| {}).unwrap();
        assert_eq!(
            (sa.systems, sa.neutron, sa.cells),
            (sb.systems, sb.neutron, sb.cells)
        );
        for name in ["stars.bin", "cells.bin", "names.bin", "byname.bin"] {
            assert_eq!(
                std::fs::read(a.path().join(name)).unwrap(),
                std::fs::read(b.path().join(name)).unwrap(),
                "{name} differed"
            );
        }
    }

    /// The v2 fixture bytes (as clients in the field hold them) still
    /// open, validate and answer queries: the reader's compatibility
    /// contract, now that the writer emits v3.
    #[test]
    fn edgx_v2_fixture_still_opens_validates_and_queries() {
        let dir = tempfile::tempdir().unwrap();
        let fixtures = [
            (
                "stars.bin",
                include_str!("../fixtures/edgx-v2/stars.hex"),
                "a271f1a0bc46fa72966092ea62dee2895e0ed771cfbd5022925d56149df01258",
            ),
            (
                "cells.bin",
                include_str!("../fixtures/edgx-v2/cells.hex"),
                "a2e68f2269211118d4c4a17fbc553612ea3da35b7f711bde8727ed8dfcff2353",
            ),
            (
                "names.bin",
                include_str!("../fixtures/edgx-v2/names.hex"),
                "a4a226e38e6aec6823b3a12517ad2a3cfa6afb79034cf2088ff23f0282bbd1ea",
            ),
            (
                "byname.bin",
                include_str!("../fixtures/edgx-v2/byname.hex"),
                "0db201e8371010e5cd3b719cf6c131cea86e18ef7c5bdd394e23b352b8e54f9e",
            ),
        ];
        for (name, hex, sha256) in fixtures {
            let bytes = fixture_hex(hex);
            assert_eq!(format!("{:x}", Sha256::digest(&bytes)), sha256, "{name} fixture corrupted");
            std::fs::write(dir.path().join(name), bytes).unwrap();
        }

        Galaxy::validate_dir(dir.path()).unwrap();
        let galaxy = Galaxy::open(dir.path()).unwrap();
        assert_eq!(galaxy.find("alpha"), Some(2));
        assert_eq!(galaxy.find("BETA"), Some(0));
        assert_eq!(galaxy.find("Sol"), Some(1));
        assert_eq!(galaxy.complete("a", 10), vec![2]);
        assert_eq!(galaxy.class_code(0), crate::StarClass::Neutron.code());
        assert_eq!(galaxy.flags(0), crate::format::FLAG_MAIN_STAR);
        assert_eq!(galaxy.companion_ls(0), None, "v2 records carry no companion distance");
    }

    /// The v3 writer's exact bytes are frozen: any unintended change to
    /// the encoding fails here before it can strand published indexes.
    #[test]
    fn edgx_v3_golden_files_are_byte_exact() {
        let dir = tempfile::tempdir().unwrap();
        import_reader(
            Box::new(std::io::Cursor::new(golden_source())),
            dir.path(),
            &mut |_| {},
        )
        .unwrap();
        let fixtures = [
            (
                "stars.bin",
                include_str!("../fixtures/edgx-v3/stars.hex"),
                "4f435417d2cfd915ed3a08defa18e155854760c094177e60a548775c80aeaeb6",
            ),
            (
                "cells.bin",
                include_str!("../fixtures/edgx-v3/cells.hex"),
                "f00af6677e51e21e62d09df3b07806bcbad3f36ac2e06b1fbe802282aa0db3d8",
            ),
            (
                "names.bin",
                include_str!("../fixtures/edgx-v3/names.hex"),
                "f74104f88fa09acb720306e40cbc032e29cbcbf3b4da61d09278047b7ae8ce67",
            ),
            (
                "byname.bin",
                include_str!("../fixtures/edgx-v3/byname.hex"),
                "0db201e8371010e5cd3b719cf6c131cea86e18ef7c5bdd394e23b352b8e54f9e",
            ),
        ];
        for (name, hex, sha256) in fixtures {
            let actual = std::fs::read(dir.path().join(name)).unwrap();
            assert_eq!(actual, fixture_hex(hex), "{name} differs from golden bytes");
            assert_eq!(format!("{:x}", Sha256::digest(&actual)), sha256);
        }
    }

    /// What the v3 writer promises, checked structurally on a fresh build:
    /// morton-keyed cells, names contiguous in record order, companion
    /// buckets populated from the bodies' distances.
    #[test]
    fn edgx_v3_build_is_morton_keyed_with_contiguous_names_and_companions() {
        let dir = tempfile::tempdir().unwrap();
        import_reader(
            Box::new(std::io::Cursor::new(golden_source())),
            dir.path(),
            &mut |_| {},
        )
        .unwrap();
        Galaxy::validate_dir(dir.path()).unwrap();
        let galaxy = Galaxy::open(dir.path()).unwrap();
        let sol = galaxy.find("Sol").unwrap();
        let ls = galaxy.companion_ls(sol).expect("Sol's fixture companion at 300 ls");
        assert!((250.0..=360.0).contains(&ls), "decoded {ls:.0} ls");
        assert!(galaxy.scoopable(sol));
        let beta = galaxy.find("beta").unwrap();
        assert_eq!(galaxy.companion_ls(beta), None);
        // Contiguity, from the reader's side: each record's name starts
        // where the previous one ended.
        let mut expected = 0u64;
        for idx in 0..galaxy.count as u32 {
            let record = galaxy.record(idx);
            assert_eq!(record.name_off, expected, "record {idx} name is out of line");
            expected += u64::from(record.name_len);
        }
        // A file with the old axis-major keys refuses v3 validation: the
        // cells check would place every record in the wrong cell. (Drop
        // the mapping first; Windows refuses writes to a mapped file.)
        drop(galaxy);
        let mut stars = std::fs::read(dir.path().join("stars.bin")).unwrap();
        stars[4..8].copy_from_slice(&2u32.to_le_bytes());
        std::fs::write(dir.path().join("stars.bin"), stars).unwrap();
        assert!(Galaxy::validate_dir(dir.path()).is_err(), "v3 keys must not validate as v2");
    }

    #[test]
    fn edgx_publication_validator_rejects_each_corrupt_companion_file() {
        let build = || {
            let dir = tempfile::tempdir().unwrap();
            import_reader(
                Box::new(std::io::Cursor::new(golden_source())),
                dir.path(),
                &mut |_| {},
            )
            .unwrap();
            dir
        };

        let dir = build();
        let path = dir.path().join("cells.bin");
        let mut bytes = std::fs::read(&path).unwrap();
        bytes.pop();
        std::fs::write(path, bytes).unwrap();
        assert!(Galaxy::validate_dir(dir.path()).is_err());

        let dir = build();
        let path = dir.path().join("byname.bin");
        let mut bytes = std::fs::read(&path).unwrap();
        bytes[4..8].copy_from_slice(&2_u32.to_le_bytes());
        std::fs::write(path, bytes).unwrap();
        assert!(Galaxy::validate_dir(dir.path()).is_err());

        let dir = build();
        let path = dir.path().join("names.bin");
        let mut bytes = std::fs::read(&path).unwrap();
        bytes[0] = 0xff;
        std::fs::write(path, bytes).unwrap();
        assert!(Galaxy::validate_dir(dir.path()).is_err());

        let dir = build();
        let path = dir.path().join("stars.bin");
        let mut bytes = std::fs::read(&path).unwrap();
        bytes.push(0);
        std::fs::write(path, bytes).unwrap();
        assert!(Galaxy::validate_dir(dir.path()).is_err());
    }

    #[test]
    #[ignore = "release-mode throughput benchmark"]
    fn synthetic_import_benchmark() {
        let count = std::env::var("EDDA_BENCH_SYSTEMS")
            .ok()
            .and_then(|v| v.parse().ok())
            .unwrap_or(100_000);
        let dump = synthetic_dump(count);
        let input_mb = dump.len() as f64 / 1e6;
        let run = |threads: usize, input: Vec<u8>| {
            let dir = tempfile::tempdir().unwrap();
            let pool = rayon::ThreadPoolBuilder::new()
                .num_threads(threads)
                .build()
                .unwrap();
            let started = std::time::Instant::now();
            let stats = pool
                .install(|| {
                    import_reader(
                        Box::new(std::io::Cursor::new(input)),
                        dir.path(),
                        &mut |_| {},
                    )
                })
                .unwrap();
            let secs = started.elapsed().as_secs_f64();
            let output: u64 = ["stars.bin", "cells.bin", "names.bin", "byname.bin"]
                .iter()
                .map(|name| std::fs::metadata(dir.path().join(name)).unwrap().len())
                .sum();
            (stats, secs, output)
        };
        let workers = std::thread::available_parallelism()
            .map(|n| n.get())
            .unwrap_or(4)
            .saturating_sub(2)
            .max(1);
        let (single, single_secs, _) = run(1, dump.clone());
        let (stats, secs, output) = run(workers, dump);
        eprintln!("BENCH systems={} workers={} single_s={:.3} parallel_s={:.3} speedup={:.2}x systems_per_sec={:.0} input_mb={:.1} parse_mb_s={:.1} output_mb={:.1} bytes_per_system={:.1}",
            stats.systems, workers, single_secs, secs, single_secs / secs,
            stats.systems as f64 / secs, input_mb, input_mb / secs,
            output as f64 / 1e6, output as f64 / stats.systems as f64);
        assert_eq!(single.systems, stats.systems);
    }
}
