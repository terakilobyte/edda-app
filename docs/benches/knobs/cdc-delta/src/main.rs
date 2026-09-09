// Item 47 bench: how many bytes would a chunk-store sync actually move
// between two published routing versions?
//
//   cdc-delta <dirA> <dirB> <file> [avg_chunk_bytes]
//
// Three measurements per file pair, one number each:
//  - exact:   differing bytes (streaming compare), the theoretical floor
//  - fixed:   bytes of 64 KiB grid blocks that differ (naive merkle leaf)
//  - cdc:     bytes of FastCDC chunks of B absent from A's chunk set
//             (what a casync-style client would download)

use std::collections::HashSet;
use std::hash::Hasher;
use std::io::Read;

const BUF: usize = 1 << 20;

fn hash64(data: &[u8]) -> u64 {
    let mut h = std::hash::DefaultHasher::new();
    h.write(data);
    h.finish()
}

fn cdc_chunks(path: &str, avg: u32) -> std::io::Result<Vec<(u64, usize)>> {
    let file = std::fs::File::open(path)?;
    let reader = std::io::BufReader::with_capacity(BUF, file);
    let mut out = Vec::new();
    for chunk in fastcdc::v2020::StreamCDC::new(reader, avg / 4, avg, avg * 4) {
        let chunk = chunk.map_err(std::io::Error::other)?;
        out.push((hash64(&chunk.data), chunk.data.len()));
    }
    Ok(out)
}

fn main() -> std::io::Result<()> {
    let args: Vec<String> = std::env::args().collect();
    let (dir_a, dir_b, name) = (&args[1], &args[2], &args[3]);
    let avg: u32 = args.get(4).map(|s| s.parse().unwrap()).unwrap_or(65536);
    let (path_a, path_b) = (format!("{dir_a}/{name}"), format!("{dir_b}/{name}"));

    // Pass 1: exact differing bytes + fixed 64 KiB grid delta.
    let mut fa = std::io::BufReader::with_capacity(BUF, std::fs::File::open(&path_a)?);
    let mut fb = std::io::BufReader::with_capacity(BUF, std::fs::File::open(&path_b)?);
    let (mut ba, mut bb) = (vec![0u8; 65536], vec![0u8; 65536]);
    let (mut total_b, mut exact, mut fixed_dirty) = (0u64, 0u64, 0u64);
    loop {
        let ra = read_full(&mut fa, &mut ba)?;
        let rb = read_full(&mut fb, &mut bb)?;
        if ra == 0 && rb == 0 {
            break;
        }
        total_b += rb as u64;
        let n = ra.min(rb);
        let diff = ba[..n].iter().zip(&bb[..n]).filter(|(x, y)| x != y).count() as u64
            + (ra.max(rb) - n) as u64;
        exact += diff;
        if diff > 0 || ra != rb {
            fixed_dirty += rb as u64;
        }
    }

    // Passes 2+3: CDC chunk sets.
    let ca = cdc_chunks(&path_a, avg)?;
    let cb = cdc_chunks(&path_b, avg)?;
    let set_a: HashSet<(u64, usize)> = ca.iter().copied().collect();
    let cdc_new: u64 = cb.iter().filter(|c| !set_a.contains(c)).map(|c| c.1 as u64).sum();

    println!(
        "{name},{total_b},{exact},{fixed_dirty},{cdc_new},{chunks_a},{chunks_b},{avg}",
        chunks_a = ca.len(),
        chunks_b = cb.len(),
    );
    Ok(())
}

fn read_full(r: &mut impl Read, buf: &mut [u8]) -> std::io::Result<usize> {
    let mut n = 0;
    while n < buf.len() {
        let k = r.read(&mut buf[n..])?;
        if k == 0 {
            break;
        }
        n += k;
    }
    Ok(n)
}
