// Item 47 bench: manufacture a same-format "next version" of a routing
// file by applying realistic churn to a copy.
//
//   mutate <input> <output> <n_updates> <n_inserts> [seed]
//
// Updates flip one byte in place at a random offset (a star-class
// correction). Inserts splice a 29-byte record at a random offset (a
// newly discovered system landing in Morton order — everything after
// it shifts). Offsets stay past the first 4 KiB so headers survive.

use std::io::{Read, Write};

struct Lcg(u64);
impl Lcg {
    fn next(&mut self) -> u64 {
        self.0 = self.0.wrapping_mul(6364136223846793005).wrapping_add(1442695040888963407);
        self.0
    }
}

fn main() -> std::io::Result<()> {
    let args: Vec<String> = std::env::args().collect();
    let (input, output) = (&args[1], &args[2]);
    let n_updates: usize = args[3].parse().unwrap();
    let n_inserts: usize = args[4].parse().unwrap();
    let seed: u64 = args.get(5).map(|s| s.parse().unwrap()).unwrap_or(47);

    let len = std::fs::metadata(input)?.len();
    let mut rng = Lcg(seed);
    let span = len - 4096;
    let mut updates: Vec<u64> = (0..n_updates).map(|_| 4096 + rng.next() % span).collect();
    let mut inserts: Vec<u64> = (0..n_inserts).map(|_| 4096 + rng.next() % span).collect();
    updates.sort_unstable();
    inserts.sort_unstable();

    let mut reader = std::io::BufReader::with_capacity(1 << 20, std::fs::File::open(input)?);
    let mut writer = std::io::BufWriter::with_capacity(1 << 20, std::fs::File::create(output)?);
    let mut buf = vec![0u8; 1 << 20];
    let (mut pos, mut ui, mut ii) = (0u64, 0usize, 0usize);
    loop {
        let n = reader.read(&mut buf)?;
        if n == 0 {
            break;
        }
        let end = pos + n as u64;
        while ui < updates.len() && updates[ui] < end {
            let off = (updates[ui] - pos) as usize;
            buf[off] = buf[off].wrapping_add(1 + (rng.next() % 255) as u8);
            ui += 1;
        }
        let mut cursor = 0usize;
        while ii < inserts.len() && inserts[ii] < end {
            let off = (inserts[ii] - pos) as usize;
            writer.write_all(&buf[cursor..off])?;
            let mut record = [0u8; 29];
            for byte in record.iter_mut() {
                *byte = (rng.next() % 256) as u8;
            }
            writer.write_all(&record)?;
            cursor = off;
            ii += 1;
        }
        writer.write_all(&buf[cursor..n])?;
        pos = end;
    }
    writer.flush()?;
    eprintln!("wrote {output}: {n_updates} updates, {n_inserts} inserts, seed {seed}");
    Ok(())
}
