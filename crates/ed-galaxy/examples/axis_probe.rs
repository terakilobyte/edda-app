//! Are the dark axis-aligned lanes in the density render real holes in
//! the index, or a cell/walk/render bug? Ground truth by brute force:
//! scan EVERY record's raw position (no cells, no walk) and count stars
//! in thin slabs on and beside the suspect planes.
//!     axis_probe <index_dir>

fn main() -> anyhow::Result<()> {
    let dir = std::path::PathBuf::from(
        std::env::args()
            .nth(1)
            .expect("usage: axis_probe <index_dir>"),
    );
    let g = ed_galaxy::Galaxy::open(&dir)?;
    // A z-band well inside the disc, away from Sol's local bubble.
    let (z_lo, z_hi) = (8_000.0f32, 12_000.0f32);
    let mut x_lane = 0u64; // |x| < 250 in the band
    let mut x_side = 0u64; // 500 < |x| < 750, same band
                           // An x-band for the horizontal lane.
    let (x_lo, x_hi) = (8_000.0f32, 12_000.0f32);
    let mut z_lane = 0u64; // |z| < 250
    let mut z_side = 0u64; // 500 < |z| < 750
    use ed_galaxy::StarClassCode as _;
    let neutron = ed_galaxy::StarClass::Neutron.code();
    let unknown = ed_galaxy::StarClass::Unknown.code();
    let mut x_lane_n = 0u64;
    let mut x_side_n = 0u64;
    let mut x_lane_u = 0u64;
    let mut x_side_u = 0u64;
    for i in 0..g.count as u32 {
        let r = g.record(i);
        let p = r.pos();
        if p[2] >= z_lo && p[2] < z_hi {
            let ax = p[0].abs();
            if ax < 250.0 {
                x_lane += 1;
                x_lane_n += (r.class == neutron) as u64;
                x_lane_u += (r.class == unknown) as u64;
            } else if (500.0..750.0).contains(&ax) {
                x_side += 1;
                x_side_n += (r.class == neutron) as u64;
                x_side_u += (r.class == unknown) as u64;
            }
        }
        if p[0] >= x_lo && p[0] < x_hi {
            let az = p[2].abs();
            if az < 250.0 {
                z_lane += 1;
            } else if (500.0..750.0).contains(&az) {
                z_side += 1;
            }
        }
    }
    println!("vertical lane   (|x|<250, z 8k..12k): {x_lane:>9}  neutron {x_lane_n:>6}  unknown {x_lane_u:>9}");
    println!("vertical ctrl   (500<|x|<750, same z): {x_side:>8}  neutron {x_side_n:>6}  unknown {x_side_u:>9}");
    println!(
        "horizontal lane (|z|<250, x 8k..12k): {z_lane:>9}   control (500<|z|<750): {z_side:>9}"
    );
    let mut lane_hist = [0u64; 32];
    let mut ctrl_hist = [0u64; 32];
    for i in 0..g.count as u32 {
        let r = g.record(i);
        let p = r.pos();
        if p[2] >= z_lo && p[2] < z_hi {
            let ax = p[0].abs();
            let slot = (r.class as usize).min(31);
            if ax < 250.0 {
                lane_hist[slot] += 1;
            } else if (500.0..750.0).contains(&ax) {
                ctrl_hist[slot] += 1;
            }
        }
    }
    println!("class histogram (code: lane / ctrl):");
    for code in 0..32 {
        if lane_hist[code] + ctrl_hist[code] > 0 {
            let class = ed_galaxy::StarClass::from_code(code as u8);
            println!(
                "  {code:>2} {class:?}: {} / {}",
                lane_hist[code], ctrl_hist[code]
            );
        }
    }
    // Who lives in the lane? Sample names from both slabs.
    let mut lane_names = 0;
    let mut ctrl_names = 0;
    for i in (0..g.count as u32).step_by(97) {
        if lane_names >= 12 && ctrl_names >= 12 {
            break;
        }
        let r = g.record(i);
        let p = r.pos();
        if p[2] < z_lo || p[2] >= z_hi {
            continue;
        }
        let ax = p[0].abs();
        if ax < 250.0 && lane_names < 12 {
            lane_names += 1;
            println!(
                "lane: {:<44} [{:>8.1} {:>7.1} {:>8.1}] id64={}",
                g.name(&r),
                p[0],
                p[1],
                p[2],
                r.id64
            );
        } else if (500.0..750.0).contains(&ax) && ctrl_names < 12 {
            ctrl_names += 1;
            println!(
                "ctrl: {:<44} [{:>8.1} {:>7.1} {:>8.1}] id64={}",
                g.name(&r),
                p[0],
                p[1],
                p[2],
                r.id64
            );
        }
    }
    Ok(())
}
