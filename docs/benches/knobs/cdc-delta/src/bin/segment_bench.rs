// Item 47 bench, part 2: what would the EDGX v4 Morton-segment layout
// ship for the same synthetic week of churn, using the REAL v45 cell
// distribution?
//
//   segment_bench <cells.bin> <stars_bytes>
//
// Reads the real cells.bin (16-byte records: key u64, start u32,
// count u32, ascending by key = stars.bin order), greedily packs
// cells into segments at several target sizes, then applies the
// bench churn (15k updates + 50k inserts = 65k change points) in two
// placements and reports dirtied-segment bytes:
//  - uniform: points uniform over records (worst case, matches the
//    CDC bench's placement)
//  - corridor: 80% of points inside 8 contiguous Morton runs of
//    5,000 cells (an expedition-swath model), 20% uniform

use std::io::Read;

struct Lcg(u64);
impl Lcg {
    fn next(&mut self) -> u64 {
        self.0 = self.0.wrapping_mul(6364136223846793005).wrapping_add(1442695040888963407);
        self.0
    }
}

fn main() -> std::io::Result<()> {
    let args: Vec<String> = std::env::args().collect();
    let cells_path = &args[1];
    let stars_bytes: u64 = args[2].parse().unwrap();

    let mut raw = Vec::new();
    std::fs::File::open(cells_path)?.read_to_end(&mut raw)?;
    let n_cells = raw.len() / 16;
    let mut starts = Vec::with_capacity(n_cells);
    for i in 0..n_cells {
        starts.push(u32::from_le_bytes(raw[i * 16 + 8..i * 16 + 12].try_into().unwrap()));
    }
    let last_count = u32::from_le_bytes(raw[(n_cells - 1) * 16 + 12..n_cells * 16].try_into().unwrap());
    let total_records = starts[n_cells - 1] as u64 + last_count as u64;
    let bytes_per_record = stars_bytes as f64 / total_records as f64;
    drop(raw);
    eprintln!("cells={n_cells} records={total_records} bytes/record={bytes_per_record:.2}");

    println!("scenario,target_bytes,segments,dirty_segments,dirty_bytes,directory_bytes");
    for target in [262144u64, 1048576, 4194304] {
        // Greedy pack: breakpoints[i] = first cell of segment i.
        let recs_per_seg = (target as f64 / bytes_per_record) as u64;
        let mut breakpoints = vec![0u32];
        let mut seg_start_record = 0u64;
        for (cell, &start) in starts.iter().enumerate() {
            if start as u64 - seg_start_record >= recs_per_seg && cell > 0 {
                breakpoints.push(cell as u32);
                seg_start_record = start as u64;
            }
        }
        let n_segs = breakpoints.len();
        let seg_of_cell = |cell: u32| breakpoints.partition_point(|&b| b <= cell) - 1;
        let cell_of_record = |rec: u32| starts.partition_point(|&s| s <= rec) - 1;
        let seg_records = |seg: usize| -> u64 {
            let first = starts[breakpoints[seg] as usize] as u64;
            let end = if seg + 1 < n_segs { starts[breakpoints[seg + 1] as usize] as u64 } else { total_records };
            end - first
        };

        for scenario in ["uniform", "corridor"] {
            let mut rng = Lcg(47);
            let mut dirty = vec![false; n_segs];
            let corridors: Vec<u64> = (0..8).map(|_| rng.next() % (n_cells as u64 - 5000)).collect();
            for _ in 0..65000u32 {
                let cell = if scenario == "corridor" && rng.next() % 10 < 8 {
                    let c = corridors[(rng.next() % 8) as usize];
                    (c + rng.next() % 5000) as u32
                } else {
                    cell_of_record((rng.next() % total_records) as u32) as u32
                };
                dirty[seg_of_cell(cell)] = true;
            }
            let dirty_segs = dirty.iter().filter(|d| **d).count();
            let dirty_bytes: u64 = (0..n_segs)
                .filter(|s| dirty[*s])
                .map(|s| (seg_records(s) as f64 * bytes_per_record) as u64)
                .sum();
            println!(
                "{scenario},{target},{n_segs},{dirty_segs},{dirty_bytes},{dir}",
                dir = n_segs * 48 // key range + hash + length per segment
            );
        }
    }
    Ok(())
}
