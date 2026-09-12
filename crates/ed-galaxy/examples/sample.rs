//! Sample stars out of an index for the 3D galaxy view: 16-byte records
//! (x, y, z as f32, class code u8, 3 bytes padding), little-endian.
//!
//!     cargo run -p ed-galaxy --example sample --release -- <index_dir> <count|all> <out.bin>
//!
//! A uniform random sample of the index is already density-weighted, so
//! the arms and the core come out right with no extra work.

use ed_galaxy::Galaxy;
use std::io::Write;
use std::path::Path;

fn main() -> anyhow::Result<()> {
    let args: Vec<String> = std::env::args().skip(1).collect();
    if args.len() < 3 {
        eprintln!("usage: sample <index_dir> <count|all> <out.bin>");
        std::process::exit(2);
    }
    let g = Galaxy::open(Path::new(&args[0]))?;
    let want: usize = if args[1] == "all" {
        g.count
    } else {
        args[1].parse()?
    };
    let n = g.count as u64;
    let mut out = std::io::BufWriter::new(std::fs::File::create(&args[2])?);
    // Deterministic stride with a scrambled start so the sample is spread
    // over the whole (cell-sorted) index rather than a prefix.
    let step = (n / want.max(1) as u64).max(1);
    let mut written = 0usize;
    let mut i: u64 = 0;
    let mut x: u64 = 0x9E37_79B9_7F4A_7C15;
    while i < n && written < want {
        let rec = g.record(i as u32);
        out.write_all(&rec.x.to_le_bytes())?;
        out.write_all(&rec.y.to_le_bytes())?;
        out.write_all(&rec.z.to_le_bytes())?;
        out.write_all(&[rec.class, 0, 0, 0])?;
        written += 1;
        // xorshift jitter within the stride so cell boundaries do not bias the sample
        x ^= x << 13;
        x ^= x >> 7;
        x ^= x << 17;
        i += if step > 1 {
            1 + (x % (2 * step - 1))
        } else {
            1
        };
    }
    out.flush()?;
    eprintln!("{written} stars written to {}", args[2]);
    Ok(())
}
