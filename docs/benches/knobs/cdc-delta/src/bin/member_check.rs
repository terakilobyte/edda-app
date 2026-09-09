// Item 47 churn ground truth: of the systems EDDN mentioned in the
// sizing window, how many are ABSENT from the published routing index?
//
//   member_check <index_dir> <cohort.csv>   (csv: name,x,y,z)
//
// Membership is spatial: a record within 0.1 ly of the reported
// position, searching the cell and its 26 neighbours.

fn main() -> anyhow::Result<()> {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let g = ed_galaxy::Galaxy::open(std::path::Path::new(&args[0]))?;
    let cell_ly = g.cell_ly;
    let csv = std::fs::read_to_string(&args[1])?;
    let (mut member, mut absent, mut bad) = (0u64, 0u64, 0u64);
    let mut absent_sample = Vec::new();
    for line in csv.lines() {
        let mut parts = line.rsplitn(4, ',');
        let (z, y, x) = (parts.next(), parts.next(), parts.next());
        let name = parts.next().unwrap_or("");
        let (Some(x), Some(y), Some(z)) = (
            x.and_then(|v| v.parse::<f32>().ok()),
            y.and_then(|v| v.parse::<f32>().ok()),
            z.and_then(|v| v.parse::<f32>().ok()),
        ) else {
            bad += 1;
            continue;
        };
        let pos = [x, y, z];
        let (cx, cy, cz) = ed_galaxy::format::cell_of_with(pos, cell_ly);
        let mut found = false;
        'search: for dx in -1..=1 {
            for dy in -1..=1 {
                for dz in -1..=1 {
                    if let Some((start, count)) = g.cell_range(cx + dx, cy + dy, cz + dz) {
                        for i in start..start + count {
                            let p = g.record(i).pos();
                            let d2 = (p[0] - x).powi(2) + (p[1] - y).powi(2) + (p[2] - z).powi(2);
                            if d2 < 0.01 {
                                found = true;
                                break 'search;
                            }
                        }
                    }
                }
            }
        }
        if found {
            member += 1;
        } else {
            absent += 1;
            if absent_sample.len() < 15 {
                absent_sample.push(name.to_string());
            }
        }
    }
    println!("member={member} absent={absent} bad={bad}");
    println!("absent sample: {absent_sample:?}");
    Ok(())
}
