//! Convert an existing v1 galaxy index to v2 without reparsing the source dump.
//!
//! Record order and names do not change, so the three auxiliary files can be
//! hard-linked (or copied when links are unavailable). Only stars.bin is
//! streamed through the v1 decoder and v2 encoder.

use anyhow::{bail, Context, Result};
use ed_galaxy::format::{Galaxy, HEADER_LEN, MAGIC, RECORD_LEN, VERSION};
use std::fs::{self, File};
use std::io::{BufWriter, Write};
use std::path::Path;
use std::time::Instant;

fn link_or_copy(from: &Path, to: &Path) -> Result<()> {
    match fs::hard_link(from, to) {
        Ok(()) => Ok(()),
        Err(_) => {
            fs::copy(from, to)
                .with_context(|| format!("copying {} to {}", from.display(), to.display()))?;
            Ok(())
        }
    }
}

fn convert(from: &Path, to: &Path) -> Result<()> {
    let galaxy = Galaxy::open(from)?;
    fs::create_dir_all(to)?;
    for name in ["cells.bin", "names.bin", "byname.bin"] {
        link_or_copy(&from.join(name), &to.join(name))?;
    }

    let tmp = to.join("stars.bin.tmp");
    let final_path = to.join("stars.bin");
    let mut out = BufWriter::with_capacity(8 << 20, File::create(&tmp)?);
    let mut header = [0u8; HEADER_LEN];
    header[0..4].copy_from_slice(MAGIC);
    header[4..8].copy_from_slice(&VERSION.to_le_bytes());
    header[8..16].copy_from_slice(&(galaxy.count as u64).to_le_bytes());
    header[16..20].copy_from_slice(&galaxy.cell_ly.to_le_bytes());
    out.write_all(&header)?;

    let started = Instant::now();
    let mut encoded = Vec::with_capacity(RECORD_LEN);
    for i in 0..galaxy.count {
        let record = galaxy.record(i as u32);
        if record.class > 0x0f || record.flags > 0x0f || record.name_off >= (1u64 << 40) {
            bail!("record {i} cannot be represented losslessly in v2: {record:?}");
        }
        encoded.clear();
        record.write_to(&mut encoded);
        out.write_all(&encoded)?;
        if i > 0 && i % 25_000_000 == 0 {
            eprintln!(
                "  {i}/{} records ({:.1} M/s)",
                galaxy.count,
                i as f64 / started.elapsed().as_secs_f64() / 1e6
            );
        }
    }
    out.flush()?;
    drop(out);
    fs::rename(&tmp, &final_path)?;
    let check = Galaxy::open(to)?;
    if check.count != galaxy.count {
        bail!(
            "converted count mismatch: {} != {}",
            check.count,
            galaxy.count
        );
    }
    eprintln!(
        "converted {} records in {:.1}s",
        galaxy.count,
        started.elapsed().as_secs_f64()
    );
    Ok(())
}

fn main() -> Result<()> {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let from = args.first().context("usage: convert_v2 FROM_DIR TO_DIR")?;
    let to = args.get(1).context("usage: convert_v2 FROM_DIR TO_DIR")?;
    let from = Path::new(from);
    let to = Path::new(to);
    if Galaxy::exists(to) || to.join("stars.bin.tmp").exists() {
        bail!(
            "destination already contains an index or partial conversion: {}",
            to.display()
        );
    }
    convert(from, to)?;
    let neutron_from = ed_galaxy::long_range::neutron_dir(from);
    if Galaxy::exists(&neutron_from) {
        convert(&neutron_from, &ed_galaxy::long_range::neutron_dir(to))?;
    }
    Ok(())
}
